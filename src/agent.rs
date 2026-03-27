use crate::client::{ApiClient, StreamEvent};
use crate::tools::Registry;
use crate::types::*;

const MAX_ITERATIONS: usize = 25;
const MAX_CONTEXT_MESSAGES: usize = 100;
const MAX_TOOL_RESULT_LEN: usize = 30_000;

const BASE_TOOLS: &str = r#"- read_file: Read file contents with line numbers (path, optional offset/limit)
- write_file: Create or overwrite files (path, content)
- edit_file: Replace exact string matches in files (path, old_string, new_string)
- bash: Execute shell commands (command, optional timeout in seconds)
- grep: Search file contents with regex (pattern, optional path/include filter)
- glob: Find files by glob pattern (pattern, optional base path)"#;

const DENEB_TOOLS: &str = r#"
- ask_deneb: Deneb AI 에이전트에게 질문 전송 (message, optional session_key). Deneb은 장기 메모리를 가진 AI 에이전트입니다.
- deneb_memory: Deneb의 장기 메모리에서 검색 (query)"#;

const GUIDELINES: &str = r#"Guidelines:
1. Read files before modifying them to understand context
2. old_string in edit_file must match exactly once in the file
3. Use markdown with language identifiers for code blocks
4. Keep bash commands focused; use timeout for long-running tasks
5. Respond in the same language as the user
6. When showing file changes, briefly explain what changed and why"#;

fn build_system_prompt(deneb_connected: bool) -> String {
    let mut prompt = String::from("You are Aurora, an AI coding assistant. You help users with software engineering tasks.\n\nAvailable tools:\n");
    prompt.push_str(BASE_TOOLS);
    if deneb_connected {
        prompt.push_str(DENEB_TOOLS);
    }
    prompt.push_str("\n\n");
    prompt.push_str(GUIDELINES);
    prompt
}

pub struct Agent {
    client: ApiClient,
    registry: Registry,
    messages: Vec<Message>,
    total_prompt_tokens: i32,
    total_completion_tokens: i32,
    deneb_connected: bool,
}

impl Agent {
    pub fn new(client: ApiClient, registry: Registry, deneb_connected: bool) -> Self {
        let system_prompt = build_system_prompt(deneb_connected);
        Self {
            client,
            registry,
            messages: vec![make_sys_msg(&system_prompt)],
            total_prompt_tokens: 0,
            total_completion_tokens: 0,
            deneb_connected,
        }
    }

    pub fn clear(&mut self) {
        let system_prompt = build_system_prompt(self.deneb_connected);
        self.messages = vec![make_sys_msg(&system_prompt)];
        self.total_prompt_tokens = 0;
        self.total_completion_tokens = 0;
    }

    pub fn message_count(&self) -> usize {
        self.messages.len().saturating_sub(1)
    }

    /// Exports conversation (excluding system prompt) as JSON.
    pub fn export_history(&self) -> Result<String, String> {
        let history: Vec<&Message> = self.messages.iter().skip(1).collect();
        serde_json::to_string_pretty(&history).map_err(|e| format!("Serialize error: {e}"))
    }

    /// Imports conversation from JSON, restoring system prompt.
    pub fn import_history(&mut self, json: &str) -> Result<usize, String> {
        let history: Vec<Message> =
            serde_json::from_str(json).map_err(|e| format!("Deserialize error: {e}"))?;
        let count = history.len();
        let system_prompt = build_system_prompt(self.deneb_connected);
        self.messages = vec![make_sys_msg(&system_prompt)];
        self.messages.extend(history);
        Ok(count)
    }

    pub async fn run(
        &mut self,
        user_message: String,
        mut on_event: impl FnMut(AgentEvent),
    ) -> Result<(), String> {
        self.messages.push(Message {
            role: "user".to_string(),
            content: Some(user_message),
            tool_calls: None,
            tool_call_id: None,
        });

        // Trim old messages if context grows too large
        self.trim_context();

        for _ in 0..MAX_ITERATIONS {
            let req = ChatRequest {
                model: String::new(),
                messages: self.messages.clone(),
                tools: Some(self.registry.definitions().to_vec()),
                tool_choice: Some("auto".to_string()),
                stream: Some(true),
                max_tokens: Some(8192),
            };

            let assistant_msg = self
                .client
                .chat_stream(&req, |evt| match evt {
                    StreamEvent::Text(t) => on_event(AgentEvent::Text(t)),
                    StreamEvent::Usage {
                        prompt,
                        completion,
                        total,
                    } => {
                        self.total_prompt_tokens += prompt;
                        self.total_completion_tokens += completion;
                        on_event(AgentEvent::Usage {
                            prompt: self.total_prompt_tokens,
                            completion: self.total_completion_tokens,
                            total,
                        });
                    }
                })
                .await?;

            self.messages.push(assistant_msg.clone());

            let tool_calls = match &assistant_msg.tool_calls {
                Some(tc) if !tc.is_empty() => tc.clone(),
                _ => {
                    on_event(AgentEvent::Done);
                    return Ok(());
                }
            };

            for tc in &tool_calls {
                on_event(AgentEvent::ToolStart {
                    name: tc.function.name.clone(),
                    args: tc.function.arguments.clone(),
                });

                let result = self
                    .registry
                    .execute(&tc.function.name, &tc.function.arguments)
                    .unwrap_or_else(|e| format!("Tool error: {e}"));

                // Truncate very large tool results to keep context manageable
                let truncated = if result.len() > MAX_TOOL_RESULT_LEN {
                    let lines: Vec<&str> = result.lines().collect();
                    let kept: String = result.chars().take(MAX_TOOL_RESULT_LEN).collect();
                    format!(
                        "{}\n\n[Truncated: showing {}/{} bytes, {}/{} lines]",
                        kept,
                        MAX_TOOL_RESULT_LEN,
                        result.len(),
                        kept.lines().count(),
                        lines.len()
                    )
                } else {
                    result.clone()
                };

                on_event(AgentEvent::ToolResult {
                    name: tc.function.name.clone(),
                    result: result.clone(),
                });

                self.messages.push(Message {
                    role: "tool".to_string(),
                    content: Some(truncated),
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                });
            }
        }

        on_event(AgentEvent::Error(format!(
            "Max iterations ({MAX_ITERATIONS}) reached"
        )));
        Ok(())
    }

    /// Trim older messages to keep context within bounds.
    /// Preserves system prompt and the most recent messages.
    fn trim_context(&mut self) {
        if self.messages.len() <= MAX_CONTEXT_MESSAGES {
            return;
        }

        let system = self.messages[0].clone();
        let keep_from = self.messages.len() - (MAX_CONTEXT_MESSAGES - 1);

        // Find a safe boundary: don't split between assistant tool_calls and tool results
        let mut safe_start = keep_from;
        for i in keep_from..self.messages.len() {
            if self.messages[i].role == "user" {
                safe_start = i;
                break;
            }
        }

        let kept = self.messages[safe_start..].to_vec();
        self.messages = vec![system];
        self.messages.extend(kept);
    }
}

fn make_sys_msg(prompt: &str) -> Message {
    Message {
        role: "system".to_string(),
        content: Some(prompt.to_string()),
        tool_calls: None,
        tool_call_id: None,
    }
}
