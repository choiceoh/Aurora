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
- grep: Search file contents with regex (pattern, optional path/include filter, case_insensitive, context_lines)
- glob: Find files by glob pattern (pattern, optional base path)
- list_dir: List directory contents with file types and sizes (path, optional recursive/max_depth)
- web_fetch: Fetch a web page and return cleaned content with metadata (url, optional include_metadata/raw)"#;

const DENEB_TOOLS: &str = r#"
- ask_deneb: Deneb AI 에이전트에게 질문 전송 (message, optional session_key). Deneb은 장기 메모리를 가진 AI 에이전트입니다.
- deneb_memory: Deneb의 장기 메모리에서 검색 (query)"#;

const GUIDELINES: &str = r#"Guidelines:
1. **Understand before acting**: Always read files and explore the project structure before making changes. Use list_dir and grep to build context.
2. **Think step by step**: For complex tasks, break the problem down. Explain your reasoning before implementing changes.
3. **Precise edits**: old_string in edit_file must match exactly once. Include enough surrounding context to ensure uniqueness.
4. **Verify your work**: After making changes, read the modified file or run tests/builds to confirm correctness.
5. **Respond in the user's language**: Match the language the user writes in.
6. **Explain changes**: When modifying code, briefly explain what changed and why.
7. **Use markdown**: Format code blocks with language identifiers for readability.
8. **Focused commands**: Keep bash commands targeted; use timeout for long-running tasks.
9. **Explore broadly, edit narrowly**: When diagnosing issues, search widely across the codebase. When fixing, make minimal targeted changes.
10. **Error recovery**: If a tool call fails, analyze the error, adjust your approach, and retry with corrected parameters."#;

fn build_system_prompt(deneb_connected: bool) -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "(unknown)".to_string());

    let mut prompt = String::from(
        "You are Aurora, an AI coding assistant powered by advanced reasoning. \
         You help users with software engineering tasks including debugging, \
         implementing features, refactoring, code review, and project exploration.\n\n",
    );

    prompt.push_str(&format!("Working directory: {cwd}\n\n"));
    prompt.push_str("Available tools:\n");
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

    /// Exports conversation (excluding system prompt) as JSON.
    pub fn export_history(&self) -> Result<String, String> {
        let history: Vec<&Message> = self.messages.iter().skip(1).collect();
        serde_json::to_string_pretty(&history).map_err(|e| format!("Serialize error: {e}"))
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

                // Truncate very large tool results — keep head + tail for context
                let truncated = if result.len() > MAX_TOOL_RESULT_LEN {
                    let lines: Vec<&str> = result.lines().collect();
                    let total_lines = lines.len();
                    // Reserve 80% for head, 20% for tail
                    let head_budget = MAX_TOOL_RESULT_LEN * 4 / 5;
                    let tail_budget = MAX_TOOL_RESULT_LEN / 5;

                    let head: String = result.chars().take(head_budget).collect();
                    let head_line_count = head.lines().count();

                    // Collect tail lines from the end
                    let mut tail_chars = 0;
                    let mut tail_start = total_lines;
                    for (i, line) in lines.iter().enumerate().rev() {
                        let line_len = line.len() + 1; // +1 for newline
                        if tail_chars + line_len > tail_budget {
                            break;
                        }
                        tail_chars += line_len;
                        tail_start = i;
                    }

                    let omitted = if tail_start > head_line_count {
                        tail_start - head_line_count
                    } else {
                        0
                    };

                    if omitted > 0 {
                        let tail: String = lines[tail_start..].join("\n");
                        format!(
                            "{}\n\n[... {omitted} lines omitted ({}/{} bytes total, {} lines) ...]\n\n{}",
                            head, result.len(), MAX_TOOL_RESULT_LEN, total_lines, tail
                        )
                    } else {
                        let kept: String = result.chars().take(MAX_TOOL_RESULT_LEN).collect();
                        format!(
                            "{}\n\n[Truncated: {}/{} bytes, {} lines total]",
                            kept, MAX_TOOL_RESULT_LEN, result.len(), total_lines
                        )
                    }
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
    /// Preserves system prompt, injects a summary of trimmed messages,
    /// and keeps the most recent messages.
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

        // Build a brief summary of trimmed conversation
        let trimmed = &self.messages[1..safe_start];
        let summary = Self::summarize_trimmed(trimmed);

        let kept = self.messages[safe_start..].to_vec();
        self.messages = vec![system];
        if !summary.is_empty() {
            self.messages.push(Message {
                role: "system".to_string(),
                content: Some(format!(
                    "[Context trimmed — summary of earlier conversation]\n{summary}"
                )),
                tool_calls: None,
                tool_call_id: None,
            });
        }
        self.messages.extend(kept);
    }

    /// Build a concise summary of trimmed messages to preserve key context.
    fn summarize_trimmed(messages: &[Message]) -> String {
        let mut user_topics = Vec::new();
        let mut tools_used = Vec::new();
        let mut files_touched = Vec::new();

        for msg in messages {
            let content = match &msg.content {
                Some(c) => c,
                None => continue,
            };

            if msg.role == "user" {
                // Extract first line as topic hint
                let first_line = content.lines().next().unwrap_or("").trim();
                if !first_line.is_empty() {
                    let truncated: String = first_line.chars().take(80).collect();
                    user_topics.push(truncated);
                }
            }

            if let Some(tool_calls) = &msg.tool_calls {
                for tc in tool_calls {
                    let name = &tc.function.name;
                    if !tools_used.contains(name) {
                        tools_used.push(name.clone());
                    }
                    // Extract file paths from tool arguments
                    if let Ok(args) = serde_json::from_str::<serde_json::Value>(&tc.function.arguments) {
                        if let Some(path) = args["path"].as_str() {
                            let path = path.to_string();
                            if !files_touched.contains(&path) {
                                files_touched.push(path);
                            }
                        }
                    }
                }
            }
        }

        let mut parts = Vec::new();
        if !user_topics.is_empty() {
            let topics: Vec<&str> = user_topics.iter().map(|s| s.as_str()).collect();
            let display: Vec<&str> = topics.iter().take(5).copied().collect();
            parts.push(format!("User topics: {}", display.join("; ")));
        }
        if !tools_used.is_empty() {
            parts.push(format!("Tools used: {}", tools_used.join(", ")));
        }
        if !files_touched.is_empty() {
            let display: Vec<&str> = files_touched.iter().take(10).map(|s| s.as_str()).collect();
            parts.push(format!("Files touched: {}", display.join(", ")));
        }
        parts.join("\n")
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
