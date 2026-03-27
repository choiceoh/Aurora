mod agent;
mod client;
mod tools;
mod types;

use agent::Agent;
use client::ApiClient;
use slint::{Model, ModelRc, SharedString, VecModel};
use std::sync::Arc;
use tokio::sync::Mutex;
use tools::Registry;
use types::AgentEvent;

slint::include_modules!();

#[tokio::main]
async fn main() {
    let api_key = std::env::var("ZHIPUAI_API_KEY").unwrap_or_else(|_| {
        eprintln!("Error: ZHIPUAI_API_KEY environment variable is not set.");
        eprintln!("Set it with: export ZHIPUAI_API_KEY=your_api_key");
        std::process::exit(1);
    });

    let model = std::env::var("AURORA_MODEL").unwrap_or_else(|_| "glm-5-turbo".to_string());
    let base_url = std::env::var("AURORA_BASE_URL").ok();

    let api_client = ApiClient::new(api_key, model.clone(), base_url);
    let registry = Registry::new();
    let agent = Arc::new(Mutex::new(Agent::new(api_client, registry)));

    let app = App::new().unwrap();
    app.set_model_name(SharedString::from(&model));

    let current_dir = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    app.set_current_dir(SharedString::from(&current_dir));
    app.set_status_text(SharedString::from("준비됨"));

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
                let mut agent = agent.lock().await;
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
            agent.lock().await.clear();
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
            let agent = agent.lock().await;
            let result = agent.export_history();
            drop(agent);

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
