use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

use aurora_common::{ClientMessage, ServerMessage};

use crate::agent::Agent;
use crate::client::ApiClient;
use crate::config::Config;
use crate::deneb::DenebClient;
use crate::tools::Registry;
use crate::types::AgentEvent;

/// Shared server state passed to each WebSocket connection.
pub struct AppState {
    pub config: Mutex<Option<Config>>,
}

impl AppState {
    pub fn new(config: Option<Config>) -> Self {
        Self {
            config: Mutex::new(config),
        }
    }
}

/// Handle a single WebSocket connection.
pub async fn handle_ws(socket: WebSocket, state: Arc<AppState>) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Channel for sending ServerMessage to the client
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMessage>();

    // Writer task: drain channel → WebSocket
    let write_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(_) => continue,
            };
            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Build agent from current config
    let agent: Arc<Mutex<Option<Agent>>> = Arc::new(Mutex::new(None));
    let cancel_token: Arc<Mutex<CancellationToken>> =
        Arc::new(Mutex::new(CancellationToken::new()));

    {
        let config_guard = state.config.lock().await;
        if let Some(config) = config_guard.as_ref() {
            let (a, deneb_status) = build_agent(config).await;
            *agent.lock().await = Some(a);
            let _ = tx.send(ServerMessage::ConfigStatus {
                needs_api_key: false,
                model: config.model.clone(),
                service: config.display_url().to_string(),
                deneb_status,
            });
        } else {
            let _ = tx.send(ServerMessage::ConfigStatus {
                needs_api_key: true,
                model: String::new(),
                service: String::new(),
                deneb_status: String::new(),
            });
        }
    }

    // Reader task: WebSocket → handle ClientMessage
    while let Some(Ok(msg)) = ws_rx.next().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            Message::Ping(_) => {
                let _ = tx.send(ServerMessage::Pong);
                continue;
            }
            _ => continue,
        };

        let client_msg: ClientMessage = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                let _ = tx.send(ServerMessage::Error {
                    message: format!("Invalid message: {e}"),
                });
                continue;
            }
        };

        match client_msg {
            ClientMessage::SendMessage { text } => {
                let agent = agent.clone();
                let tx = tx.clone();
                // Create a fresh cancellation token for this request
                let token = CancellationToken::new();
                *cancel_token.lock().await = token.clone();
                // Run agent in a spawned task so we can keep reading WS
                tokio::spawn(async move {
                    let mut guard = agent.lock().await;
                    let Some(agent) = guard.as_mut() else {
                        let _ = tx.send(ServerMessage::Error {
                            message: "API 키를 먼저 설정하세요".to_string(),
                        });
                        return;
                    };

                    let tx2 = tx.clone();
                    let result = agent
                        .run(text, token, |evt| {
                            let msg = agent_event_to_server_message(evt);
                            let _ = tx2.send(msg);
                        })
                        .await;

                    if let Err(e) = result {
                        let _ = tx.send(ServerMessage::Error { message: e });
                    }
                });
            }
            ClientMessage::StopGeneration => {
                cancel_token.lock().await.cancel();
                let _ = tx.send(ServerMessage::Done);
            }
            ClientMessage::ClearChat => {
                let mut guard = agent.lock().await;
                if let Some(a) = guard.as_mut() {
                    a.clear();
                }
                let _ = tx.send(ServerMessage::ChatCleared);
            }
            ClientMessage::SaveSession => {
                let guard = agent.lock().await;
                match guard.as_ref() {
                    Some(a) => match a.export_history() {
                        Ok(json) => match save_session_file(&json) {
                            Ok(path) => {
                                let _ = tx.send(ServerMessage::SessionSaved { path });
                            }
                            Err(e) => {
                                let _ = tx.send(ServerMessage::Error { message: e });
                            }
                        },
                        Err(e) => {
                            let _ = tx.send(ServerMessage::Error { message: e });
                        }
                    },
                    None => {
                        let _ = tx.send(ServerMessage::Error {
                            message: "에이전트가 초기화되지 않았습니다".to_string(),
                        });
                    }
                }
            }
            ClientMessage::SetApiKey { key } => {
                match Config::init_with_key(key) {
                    Ok(config) => {
                        let (a, deneb_status) = build_agent(&config).await;
                        *agent.lock().await = Some(a);
                        let _ = tx.send(ServerMessage::ConfigStatus {
                            needs_api_key: false,
                            model: config.model.clone(),
                            service: config.display_url().to_string(),
                            deneb_status,
                        });
                        // Update shared config
                        *state.config.lock().await = Some(config);
                    }
                    Err(e) => {
                        let _ = tx.send(ServerMessage::Error {
                            message: format!("저장 실패: {e}"),
                        });
                    }
                }
            }
            ClientMessage::Ping => {
                let _ = tx.send(ServerMessage::Pong);
            }
        }
    }

    write_task.abort();
}

fn agent_event_to_server_message(evt: AgentEvent) -> ServerMessage {
    match evt {
        AgentEvent::Text(content) => ServerMessage::Text { content },
        AgentEvent::ToolStart { name, args } => ServerMessage::ToolStart { name, args },
        AgentEvent::ToolResult { name, result } => ServerMessage::ToolResult { name, result },
        AgentEvent::Usage {
            prompt,
            completion,
            total,
        } => ServerMessage::Usage {
            prompt,
            completion,
            total,
        },
        AgentEvent::Done => ServerMessage::Done,
        AgentEvent::Error(message) => ServerMessage::Error { message },
    }
}

async fn build_agent(config: &Config) -> (Agent, String) {
    let deneb_client = config
        .deneb_url
        .as_ref()
        .map(|url| Arc::new(DenebClient::new(url)));
    let deneb_connected = deneb_client.is_some();

    let deneb_status = if let Some(ref dc) = deneb_client {
        match dc.health_check().await {
            Ok(true) => "연결됨".to_string(),
            _ => "연결 실패".to_string(),
        }
    } else {
        String::new()
    };

    let api_client = ApiClient::from_config(config);
    let registry = Registry::new(deneb_client);
    let agent = Agent::new(api_client, registry, deneb_connected);

    (agent, deneb_status)
}

fn save_session_file(json: &str) -> Result<String, String> {
    let dir = dirs::home_dir()
        .ok_or("Cannot find home directory")?
        .join(".aurora")
        .join("sessions");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create dir: {e}"))?;

    let timestamp = {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    };
    let path = dir.join(format!("{timestamp}.json"));
    std::fs::write(&path, json).map_err(|e| format!("Write error: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}
