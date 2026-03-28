use crate::config::Config;
use crate::types::*;
use futures_util::StreamExt;
use reqwest::Client;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const MAX_RETRIES: usize = 3;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

pub struct ApiClient {
    client: Client,
    api_key: String,
    pub model: String,
    base_url: String,
}

impl ApiClient {
    pub fn from_config(config: &Config) -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .pool_max_idle_per_host(4)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            api_key: config.api_key.clone(),
            model: config.model.clone(),
            base_url: config.base_url.clone(),
        }
    }

    pub async fn chat_stream(
        &self,
        req: &ChatRequest,
        cancel: &CancellationToken,
        mut on_event: impl FnMut(StreamEvent),
    ) -> Result<Message, String> {
        let url = format!("{}/chat/completions", self.base_url);

        let mut body = req.clone();
        body.model.clone_from(&self.model);
        body.stream = Some(true);

        // Retry logic with exponential backoff
        let response = {
            let mut last_err = String::new();
            let mut resp = None;
            for attempt in 0..MAX_RETRIES {
                if attempt > 0 {
                    let delay = Duration::from_millis(500 * 2u64.pow(attempt as u32));
                    tokio::time::sleep(delay).await;
                }
                match self
                    .client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("Content-Type", "application/json")
                    .timeout(REQUEST_TIMEOUT)
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(r) => {
                        resp = Some(r);
                        break;
                    }
                    Err(e) => {
                        last_err = format!("Request failed (attempt {}): {e}", attempt + 1);
                        if e.is_timeout() || e.is_connect() {
                            continue;
                        }
                        return Err(last_err);
                    }
                }
            }
            resp.ok_or(last_err)?
        };

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("API error (HTTP {status}): {text}"));
        }

        let mut stream = response.bytes_stream();
        let mut buf = String::new();
        let mut content = String::new();
        let mut tool_calls: Vec<ToolCallBuilder> = Vec::new();

        while let Some(chunk) = tokio::select! {
            chunk = stream.next() => chunk,
            _ = cancel.cancelled() => None,
        } {
            let chunk = chunk.map_err(|e| format!("Stream error: {e}"))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));

            // Efficient line extraction: drain processed portion
            while let Some(line_end) = buf.find('\n') {
                let line: String = buf.drain(..=line_end).collect();
                let line = line.trim();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                let data = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                    .unwrap_or(line)
                    .trim();

                if data == "[DONE]" {
                    break;
                }

                let parsed: ChatResponse = match serde_json::from_str(data) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                if let Some(choice) = parsed.choices.first()
                    && let Some(delta) = &choice.delta {
                    if let Some(text) = &delta.content
                        && !text.is_empty() {
                            content.push_str(text);
                            on_event(StreamEvent::Text(text.clone()));
                        }

                    if let Some(tcs) = &delta.tool_calls {
                        for tc in tcs {
                            let idx = tc.index.unwrap_or(0) as usize;
                            while tool_calls.len() <= idx {
                                tool_calls.push(ToolCallBuilder::default());
                            }
                            let builder = &mut tool_calls[idx];
                            if !tc.id.is_empty() {
                                builder.id.clone_from(&tc.id);
                            }
                            if !tc.function.name.is_empty() {
                                builder.name.clone_from(&tc.function.name);
                            }
                            if !tc.function.arguments.is_empty() {
                                builder.arguments.push_str(&tc.function.arguments);
                            }
                        }
                    }
                }

                if let Some(usage) = &parsed.usage {
                    on_event(StreamEvent::Usage {
                        prompt: usage.prompt_tokens.unwrap_or(0),
                        completion: usage.completion_tokens.unwrap_or(0),
                        total: usage.total_tokens.unwrap_or(0),
                    });
                }
            }
        }

        let tool_calls_result: Option<Vec<ToolCall>> = if tool_calls.is_empty() {
            None
        } else {
            Some(
                tool_calls
                    .into_iter()
                    .map(|b| ToolCall {
                        id: b.id,
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: b.name,
                            arguments: b.arguments,
                        },
                        index: None,
                    })
                    .collect(),
            )
        };

        Ok(Message {
            role: "assistant".to_string(),
            content: if content.is_empty() {
                None
            } else {
                Some(content)
            },
            tool_calls: tool_calls_result,
            tool_call_id: None,
        })
    }
}

#[derive(Debug, Default)]
struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug)]
pub enum StreamEvent {
    Text(String),
    Usage { prompt: i32, completion: i32, total: i32 },
}
