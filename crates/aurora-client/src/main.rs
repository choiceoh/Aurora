mod ws_client;

use aurora_common::{ClientMessage, ServerMessage};
use slint::{Model, ModelRc, SharedString, VecModel};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

slint::include_modules!();

#[tokio::main]
async fn main() {
    let app = App::new().unwrap();

    // Shared sender to server (set after connection)
    let ws_tx: Arc<Mutex<Option<mpsc::UnboundedSender<ClientMessage>>>> =
        Arc::new(Mutex::new(None));

    // Load saved server URL
    let saved_url = load_client_config();
    if let Some(ref url) = saved_url {
        app.set_status_text(SharedString::from(&format!("저장된 서버: {url}")));
    }

    // ─── Connect to server ───
    let app_weak = app.as_weak();
    let ws_tx_clone = ws_tx.clone();
    app.on_connect_server(move |url| {
        let url = url.to_string().trim().to_string();
        if url.is_empty() {
            return;
        }

        let app_weak = app_weak.clone();
        let ws_tx = ws_tx_clone.clone();

        // Save URL for next time
        let _ = save_client_config(&url);

        if let Some(app) = app_weak.upgrade() {
            app.set_status_text(SharedString::from("연결 중..."));
        }

        tokio::spawn(async move {
            match ws_client::connect(&url).await {
                Ok(handle) => {
                    *ws_tx.lock().await = Some(handle.tx);

                    let aw = app_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = aw.upgrade() {
                            app.set_needs_server_url(false);
                            app.set_connection_status(SharedString::from("connected"));
                            app.set_status_text(SharedString::from("서버 연결됨, 설정 확인 중..."));
                        }
                    });

                    // Start receiving messages
                    spawn_receiver(app_weak, handle.rx);
                }
                Err(e) => {
                    let aw = app_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = aw.upgrade() {
                            app.set_status_text(SharedString::from(&format!("연결 실패: {e}")));
                        }
                    });
                }
            }
        });
    });

    // ─── Send message ───
    let app_weak = app.as_weak();
    let ws_tx_clone = ws_tx.clone();
    app.on_send_message(move |text| {
        let text = text.to_string();
        if text.is_empty() {
            return;
        }

        if let Some(app) = app_weak.upgrade() {
            push_message(&app, "user", &text, false, "");
            app.set_is_streaming(true);
            app.set_status_text(SharedString::from("생성 중..."));
            app.set_streaming_text(SharedString::default());
        }

        let ws_tx = ws_tx_clone.clone();
        tokio::spawn(async move {
            if let Some(tx) = ws_tx.lock().await.as_ref() {
                let _ = tx.send(ClientMessage::SendMessage { text });
            }
        });
    });

    // ─── Clear chat ───
    let app_weak = app.as_weak();
    let ws_tx_clone = ws_tx.clone();
    app.on_clear_chat(move || {
        let app_weak = app_weak.clone();
        let ws_tx = ws_tx_clone.clone();
        tokio::spawn(async move {
            if let Some(tx) = ws_tx.lock().await.as_ref() {
                let _ = tx.send(ClientMessage::ClearChat);
            }
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = app_weak.upgrade() {
                    app.set_messages(ModelRc::new(VecModel::from(Vec::<ChatMessage>::new())));
                    app.set_streaming_text(SharedString::default());
                    app.set_usage_text(SharedString::default());
                    app.set_status_text(SharedString::from("대화 초기화됨"));
                }
            });
        });
    });

    // ─── Save session ───
    let ws_tx_clone = ws_tx.clone();
    app.on_save_session(move || {
        let ws_tx = ws_tx_clone.clone();
        tokio::spawn(async move {
            if let Some(tx) = ws_tx.lock().await.as_ref() {
                let _ = tx.send(ClientMessage::SaveSession);
            }
        });
    });

    // ─── Submit API key ───
    let ws_tx_clone = ws_tx.clone();
    app.on_submit_api_key(move |key| {
        let key = key.to_string().trim().to_string();
        if key.is_empty() {
            return;
        }
        let ws_tx = ws_tx_clone.clone();
        tokio::spawn(async move {
            if let Some(tx) = ws_tx.lock().await.as_ref() {
                let _ = tx.send(ClientMessage::SetApiKey { key });
            }
        });
    });

    // If we have a saved URL, auto-connect
    if let Some(url) = saved_url {
        let app_weak = app.as_weak();
        let ws_tx_clone = ws_tx.clone();
        tokio::spawn(async move {
            match ws_client::connect(&url).await {
                Ok(handle) => {
                    *ws_tx_clone.lock().await = Some(handle.tx);
                    let aw = app_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = aw.upgrade() {
                            app.set_needs_server_url(false);
                            app.set_connection_status(SharedString::from("connected"));
                            app.set_status_text(SharedString::from("서버 연결됨"));
                        }
                    });
                    spawn_receiver(app_weak, handle.rx);
                }
                Err(_) => {
                    // Auto-connect failed, show manual input
                    let aw = app_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = aw.upgrade() {
                            app.set_status_text(SharedString::from(
                                &format!("자동 연결 실패 — 서버 주소를 확인하세요 ({url})"),
                            ));
                        }
                    });
                }
            }
        });
    }

    app.run().unwrap();
}

/// Spawn a task that receives ServerMessages and updates the Slint UI.
fn spawn_receiver(
    app_weak: slint::Weak<App>,
    mut rx: mpsc::UnboundedReceiver<ServerMessage>,
) {
    let streaming_buf = Arc::new(Mutex::new(String::new()));

    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let app_weak = app_weak.clone();
            let buf = streaming_buf.clone();

            match msg {
                ServerMessage::Text { content } => {
                    buf.lock().await.push_str(&content);
                    let text = buf.lock().await.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            app.set_streaming_text(SharedString::from(&text));
                        }
                    });
                }
                ServerMessage::ToolStart { name, args } => {
                    // Finalize streaming text before tool block
                    let pending = {
                        let mut b = buf.lock().await;
                        let t = b.clone();
                        b.clear();
                        t
                    };
                    let summary = safe_truncate(&args, 120);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            if !pending.is_empty() {
                                push_message(&app, "assistant", &pending, false, "");
                                app.set_streaming_text(SharedString::default());
                            }
                            push_message(&app, "tool", &format!("⏳ {summary}"), true, &name);
                        }
                    });
                }
                ServerMessage::ToolResult { name, result } => {
                    let short = safe_truncate(&result, 800);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            push_message(&app, "tool", &short, true, &format!("{name} ✅"));
                        }
                    });
                }
                ServerMessage::Usage {
                    prompt,
                    completion,
                    total,
                } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            app.set_usage_text(SharedString::from(&format!(
                                "📥 {prompt}  📤 {completion}  합계: {total} tokens"
                            )));
                        }
                    });
                }
                ServerMessage::Done => {
                    let final_text = {
                        let mut b = buf.lock().await;
                        let t = b.clone();
                        b.clear();
                        t
                    };
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            if !final_text.is_empty() {
                                push_message(&app, "assistant", &final_text, false, "");
                            }
                            app.set_streaming_text(SharedString::default());
                            app.set_is_streaming(false);
                            app.set_status_text(SharedString::from("준비됨"));
                        }
                    });
                }
                ServerMessage::Error { message } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            push_message(
                                &app,
                                "assistant",
                                &format!("⚠️ 오류: {message}"),
                                false,
                                "",
                            );
                            app.set_streaming_text(SharedString::default());
                            app.set_is_streaming(false);
                            app.set_status_text(SharedString::from("오류 발생"));
                        }
                    });
                }
                ServerMessage::SessionSaved { path } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            app.set_status_text(SharedString::from(&format!(
                                "세션 저장됨: {path}"
                            )));
                        }
                    });
                }
                ServerMessage::ChatCleared => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            app.set_messages(ModelRc::new(VecModel::from(
                                Vec::<ChatMessage>::new(),
                            )));
                            app.set_streaming_text(SharedString::default());
                            app.set_usage_text(SharedString::default());
                            app.set_status_text(SharedString::from("대화 초기화됨"));
                        }
                    });
                }
                ServerMessage::ConfigStatus {
                    needs_api_key,
                    model,
                    service,
                    deneb_status,
                } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            app.set_needs_api_key(needs_api_key);
                            if !needs_api_key {
                                app.set_model_name(SharedString::from(&model));
                                app.set_service_name(SharedString::from(&service));
                                let status = if deneb_status.is_empty() {
                                    "준비됨".to_string()
                                } else {
                                    format!("준비됨 (Deneb {deneb_status})")
                                };
                                app.set_status_text(SharedString::from(&status));
                            }
                        }
                    });
                }
                ServerMessage::Pong => {}
            }
        }

        // WebSocket closed — update UI
        let aw = app_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = aw.upgrade() {
                app.set_connection_status(SharedString::from("disconnected"));
                app.set_status_text(SharedString::from("서버 연결 끊김"));
                app.set_is_streaming(false);
            }
        });
    });
}

// ─── Helpers ───

fn push_message(app: &App, role: &str, content: &str, is_tool: bool, tool_name: &str) {
    let messages = app.get_messages();
    let vec_model = clone_model(&messages);
    vec_model.push(ChatMessage {
        role: SharedString::from(role),
        content: SharedString::from(content),
        is_tool,
        tool_name: SharedString::from(tool_name),
    });
    let count = vec_model.row_count() as i32;
    app.set_messages(ModelRc::from(vec_model));
    app.set_msg_count(count);
}

fn clone_model(model: &ModelRc<ChatMessage>) -> std::rc::Rc<VecModel<ChatMessage>> {
    let items: Vec<ChatMessage> = (0..model.row_count())
        .filter_map(|i| model.row_data(i))
        .collect();
    std::rc::Rc::new(VecModel::from(items))
}

fn safe_truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() > 8 {
        let preview: String = lines[..6].join("\n");
        return format!("{preview}\n… ({} more lines)", lines.len() - 6);
    }
    let truncated: String = s.chars().take(max_chars).collect();
    format!("{truncated}…")
}

// ─── Client Config ───

fn client_config_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".aurora")
        .join("client.json")
}

fn load_client_config() -> Option<String> {
    let path = client_config_path();
    let data = std::fs::read_to_string(&path).ok()?;
    let config: serde_json::Value = serde_json::from_str(&data).ok()?;
    config
        .get("server_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn save_client_config(server_url: &str) -> Result<(), String> {
    let path = client_config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{e}"))?;
    }
    let json = serde_json::json!({ "server_url": server_url });
    let data = serde_json::to_string_pretty(&json).map_err(|e| format!("{e}"))?;
    std::fs::write(&path, data).map_err(|e| format!("{e}"))?;
    Ok(())
}
