use super::Registry;
use serde_json::{json, Value};
use std::process::Command;
use std::time::{Duration, Instant};

const MAX_OUTPUT: usize = 100_000;
const DEFAULT_TIMEOUT: u64 = 30;
const MAX_TIMEOUT: u64 = 300;

pub fn register(reg: &mut Registry) {
    reg.register_tool(
        "bash",
        "Execute a bash command and return its output. Use for builds, tests, git, etc.",
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Bash command to execute" },
                "timeout": { "type": "integer", "description": "Timeout in seconds (default: 30, max: 300)" }
            },
            "required": ["command"]
        }),
        Box::new(execute_bash),
    );
}

fn execute_bash(args: Value) -> Result<String, String> {
    let command = args["command"].as_str().ok_or("command is required")?;
    if command.trim().is_empty() {
        return Err("command cannot be empty".to_string());
    }

    let timeout_secs = args["timeout"]
        .as_i64()
        .unwrap_or(DEFAULT_TIMEOUT as i64)
        .max(1) as u64;
    let timeout_secs = timeout_secs.min(MAX_TIMEOUT);
    let timeout = Duration::from_secs(timeout_secs);

    let mut child = Command::new("bash")
        .arg("-c")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot spawn bash: {e}"))?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(format!("[Command timed out after {timeout_secs}s]"));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("Wait error: {e}")),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Output error: {e}"))?;

    let mut result = String::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !stdout.is_empty() {
        if stdout.len() > MAX_OUTPUT {
            let truncated: String = stdout.chars().take(MAX_OUTPUT).collect();
            result.push_str(&format!("[stdout truncated to {MAX_OUTPUT} chars]\n"));
            result.push_str(&truncated);
        } else {
            result.push_str(&stdout);
        }
    }

    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("STDERR:\n");
        if stderr.len() > MAX_OUTPUT {
            let truncated: String = stderr.chars().take(MAX_OUTPUT).collect();
            result.push_str(&format!("[stderr truncated to {MAX_OUTPUT} chars]\n"));
            result.push_str(&truncated);
        } else {
            result.push_str(&stderr);
        }
    }

    if !output.status.success() {
        result.push_str(&format!(
            "\n[Exit code: {}]",
            output.status.code().unwrap_or(-1)
        ));
    }

    if result.is_empty() {
        Ok("(no output)".to_string())
    } else {
        Ok(result)
    }
}
