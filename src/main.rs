mod agent;
mod client;
mod config;
mod deneb;
mod tools;
mod types;

use agent::Agent;
use client::ApiClient;
use config::Config;
use slint::{Model, ModelRc, SharedString, VecModel};
use std::sync::Arc;
use tokio::sync::Mutex;
use tools::Registry;
use types::AgentEvent;

slint::include_modules!();

#[tokio::main]
async fn main() {
    let app = App::new().unwrap();

    let current_dir = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    app.set_current_dir(SharedString::from(&current_dir));

    // 설정 로드 시도
    let agent: Arc<Mutex<Option<Agent>>> = Arc::new(Mutex::new(None));

    if let Some(config) = Config::load() {
        // 설정 있음 → 바로 시작
        app.set_needs_api_key(false);
        app.set_model_name(SharedString::from(&config.model));
        app.set_service_name(SharedString::from(config.display_url()));

        let deneb_client = config.deneb_url.as_ref().map(|url| {
            Arc::new(deneb::DenebClient::new(url))
        });
        let deneb_connected = deneb_client.is_some();

        let api_client = ApiClient::from_config(&config);
        let registry = Registry::new(deneb_client.clone());
        *agent.blocking_lock() = Some(Agent::new(api_client, registry, deneb_connected));

        if let Some(dc) = deneb_client {
            let app_weak = app.as_weak();
            tokio::spawn(async move {
                match dc.health_check().await {
                    Ok(true) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(app) = app_weak.upgrade() {
                                app.set_status_text(SharedString::from("준비됨 (Deneb 연결됨)"));
                            }
                        });
                    }
                    _ => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(app) = app_weak.upgrade() {
                                app.set_status_text(SharedString::from("준비됨 (Deneb 연결 실패)"));
                            }
                        });
                    }
                }
            });
        } else {
            app.set_status_text(SharedString::from("준비됨"));
        }
    } else {
        // 설정 없음 → API 키 입력 화면
        app.set_needs_api_key(true);
    }

    // ─── API 키 제출 ───
    let app_weak = app.as_weak();
    let agent_for_setup = agent.clone();
    app.on_submit_api_key(move |key| {
        let key = key.to_string().trim().to_string();
        if key.is_empty() {
            return;
        }

        match Config::init_with_key(key) {
            Ok(config) => {
                let deneb_client = config.deneb_url.as_ref().map(|url| {
                    Arc::new(deneb::DenebClient::new(url))
                });
                let deneb_connected = deneb_client.is_some();

                let api_client = ApiClient::from_config(&config);
                let registry = Registry::new(deneb_client.clone());
                let new_agent = Agent::new(api_client, registry, deneb_connected);
                *agent_for_setup.blocking_lock() = Some(new_agent);

                if let Some(app) = app_weak.upgrade() {
                    app.set_model_name(SharedString::from(&config.model));
                    app.set_service_name(SharedString::from(config.display_url()));
                    app.set_needs_api_key(false);
                    if deneb_connected {
                        app.set_status_text(SharedString::from("준비됨 (Deneb 설정됨)"));
                    } else {
                        app.set_status_text(SharedString::from("준비됨"));
                    }
                }
            }
            Err(e) => {
                if let Some(app) = app_weak.upgrade() {
                    app.set_status_text(SharedString::from(&format!("저장 실패: {e}")));
                }
            }
        }
    });

    // ─── Send message ───
    let app_weak = app.as_weak();
    let agent_clone = agent.clone();

    app.on_send_message(move |text| {
        let text = text.to_string();
        if text.is_empty() {
            return;
        }

        let app_weak = app_weak.clone();
        let agent = agent_clone.clone();

        // Add user message to UI immediately
        if let Some(app) = app_weak.upgrade() {
            push_message(
                &app,
                "user",
                &text,
                false,
                "",
            );
            app.set_is_streaming(true);
            app.set_status_text(SharedString::from("생성 중..."));
            app.set_streaming_text(SharedString::default());
        }

        tokio::spawn(async move {
            let mut streaming_buf = String::new();

            let result = {
                let mut guard = agent.lock().await;
                let Some(agent) = guard.as_mut() else {
                    let aw = app_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = aw.upgrade() {
                            app.set_is_streaming(false);
                            app.set_status_text(SharedString::from("API 키를 먼저 설정하세요"));
                        }
                    });
                    return;
                };
                agent
                    .run(text, |evt| {
                        let app_weak = app_weak.clone();
                        match evt {
                            AgentEvent::Text(t) => {
                                streaming_buf.push_str(&t);
                                let buf = streaming_buf.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(app) = app_weak.upgrade() {
                                        app.set_streaming_text(SharedString::from(&buf));
                                    }
                                });
                            }
                            AgentEvent::ToolStart { name, args } => {
                                // Finalize any streaming text before tool block
                                if !streaming_buf.is_empty() {
                                    let text = streaming_buf.clone();
                                    streaming_buf.clear();
                                    let aw = app_weak.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(app) = aw.upgrade() {
                                            push_message(&app, "assistant", &text, false, "");
                                            app.set_streaming_text(SharedString::default());
                                        }
                                    });
                                }

                                let summary = safe_truncate(&args, 120);
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(app) = app_weak.upgrade() {
                                        push_message(
                                            &app,
                                            "tool",
                                            &format!("⏳ {summary}"),
                                            true,
                                            &name,
                                        );
                                    }
                                });
                            }
                            AgentEvent::ToolResult { name, result } => {
                                let short = safe_truncate(&result, 800);
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(app) = app_weak.upgrade() {
                                        push_message(
                                            &app,
                                            "tool",
                                            &short,
                                            true,
                                            &format!("{name} ✅"),
                                        );
                                    }
                                });
                            }
                            AgentEvent::Usage {
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
                            AgentEvent::Done => {
                                let final_text = streaming_buf.clone();
                                streaming_buf.clear();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(app) = app_weak.upgrade() {
                                        if !final_text.is_empty() {
                                            push_message(
                                                &app,
                                                "assistant",
                                                &final_text,
                                                false,
                                                "",
                                            );
                                        }
                                        app.set_streaming_text(SharedString::default());
                                        app.set_is_streaming(false);
                                        app.set_status_text(SharedString::from("준비됨"));
                                    }
                                });
                            }
                            AgentEvent::Error(e) => {
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(app) = app_weak.upgrade() {
                                        push_message(
                                            &app,
                                            "assistant",
                                            &format!("⚠️ 오류: {e}"),
                                            false,
                                            "",
                                        );
                                        app.set_streaming_text(SharedString::default());
                                        app.set_is_streaming(false);
                                        app.set_status_text(SharedString::from("오류 발생"));
                                    }
                                });
                            }
                        }
                    })
                    .await
            };

            if let Err(e) = result {
                let aw = app_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = aw.upgrade() {
                        push_message(&app, "assistant", &format!("❌ {e}"), false, "");
                        app.set_streaming_text(SharedString::default());
                        app.set_is_streaming(false);
                        app.set_status_text(SharedString::from("오류 발생"));
                    }
                });
            }
        });
    });

    // ─── Clear chat ───
    let app_weak = app.as_weak();
    let agent_clone = agent.clone();
    app.on_clear_chat(move || {
        let agent = agent_clone.clone();
        let app_weak = app_weak.clone();

        tokio::spawn(async move {
            if let Some(a) = agent.lock().await.as_mut() {
                a.clear();
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
    let app_weak = app.as_weak();
    let agent_clone = agent.clone();
    app.on_save_session(move || {
        let agent = agent_clone.clone();
        let app_weak = app_weak.clone();

        tokio::spawn(async move {
            let guard = agent.lock().await;
            let result = match guard.as_ref() {
                Some(a) => a.export_history(),
                None => Err("에이전트가 초기화되지 않았습니다".to_string()),
            };
            drop(guard);

            match result {
                Ok(json) => {
                    let save_result = save_session_file(&json);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            match save_result {
                                Ok(path) => {
                                    app.set_status_text(SharedString::from(
                                        &format!("세션 저장됨: {path}"),
                                    ));
                                }
                                Err(e) => {
                                    app.set_status_text(SharedString::from(
                                        &format!("저장 실패: {e}"),
                                    ));
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            app.set_status_text(SharedString::from(&format!("저장 실패: {e}")));
                        }
                    });
                }
            }
        });
    });

    app.run().unwrap();
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

/// UTF-8 safe truncation that never panics on multi-byte boundaries.
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

fn save_session_file(json: &str) -> Result<String, String> {
    let dir = dirs::home_dir()
        .ok_or("Cannot find home directory")?
        .join(".aurora")
        .join("sessions");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create dir: {e}"))?;

    let timestamp = chrono_now();
    let path = dir.join(format!("{timestamp}.json"));
    std::fs::write(&path, json).map_err(|e| format!("Write error: {e}"))?;

    Ok(path.to_string_lossy().to_string())
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{secs}")
}
