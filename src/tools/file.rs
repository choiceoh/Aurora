use super::Registry;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_READ_SIZE: u64 = 10 * 1024 * 1024; // 10MB

pub fn register(reg: &mut Registry) {
    reg.register_tool(
        "read_file",
        "Read the contents of a file. Returns file content with line numbers.",
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path to read" },
                "offset": { "type": "integer", "description": "Starting line number (1-based, optional)" },
                "limit": { "type": "integer", "description": "Max lines to read (optional, default: 2000)" }
            },
            "required": ["path"]
        }),
        Box::new(read_file),
    );

    reg.register_tool(
        "write_file",
        "Create or overwrite a file with given content.",
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path to write to" },
                "content": { "type": "string", "description": "Content to write" }
            },
            "required": ["path", "content"]
        }),
        Box::new(write_file),
    );

    reg.register_tool(
        "edit_file",
        "Edit a file by replacing an exact string match. old_string must match exactly once.",
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path to edit" },
                "old_string": { "type": "string", "description": "Exact string to find" },
                "new_string": { "type": "string", "description": "Replacement string" }
            },
            "required": ["path", "old_string", "new_string"]
        }),
        Box::new(edit_file),
    );
}

fn resolve_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("Path cannot be empty".to_string());
    }
    let p = Path::new(path);
    let resolved = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("Cannot get working directory: {e}"))?
            .join(p)
    };
    // Canonicalize existing paths to resolve symlinks, but allow new paths
    if resolved.exists() {
        resolved
            .canonicalize()
            .map_err(|e| format!("Path resolution error: {e}"))
    } else {
        Ok(resolved)
    }
}

fn check_file_size(path: &Path) -> Result<(), String> {
    if let Ok(meta) = fs::metadata(path) {
        if meta.len() > MAX_READ_SIZE {
            return Err(format!(
                "File too large ({:.1}MB > {:.0}MB limit). Use offset/limit to read portions.",
                meta.len() as f64 / 1_048_576.0,
                MAX_READ_SIZE as f64 / 1_048_576.0
            ));
        }
    }
    Ok(())
}

fn read_file(args: Value) -> Result<String, String> {
    let path = args["path"].as_str().ok_or("path is required")?;
    let offset = args["offset"].as_i64().unwrap_or(0).max(0) as usize;
    let limit = args["limit"].as_i64().unwrap_or(2000).max(1) as usize;

    let resolved = resolve_path(path)?;
    check_file_size(&resolved)?;

    let content =
        fs::read_to_string(&resolved).map_err(|e| format!("Cannot read {path}: {e}"))?;

    let lines: Vec<&str> = content.lines().collect();
    let start = if offset > 0 {
        (offset - 1).min(lines.len())
    } else {
        0
    };
    let end = (start + limit).min(lines.len());

    let mut result = String::with_capacity((end - start) * 80);
    for (i, line) in lines[start..end].iter().enumerate() {
        result.push_str(&format!("{:4}\t{}\n", start + i + 1, line));
    }

    if end < lines.len() {
        result.push_str(&format!(
            "\n[Showing lines {}-{} of {} total]",
            start + 1,
            end,
            lines.len()
        ));
    }
    Ok(result)
}

fn write_file(args: Value) -> Result<String, String> {
    let path = args["path"].as_str().ok_or("path is required")?;
    let content = args["content"].as_str().ok_or("content is required")?;

    let resolved = resolve_path(path)?;
    if let Some(parent) = resolved.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Cannot create directory: {e}"))?;
    }

    let existed = resolved.exists();
    fs::write(&resolved, content).map_err(|e| format!("Cannot write {path}: {e}"))?;

    Ok(format!(
        "{} {path} ({} bytes)",
        if existed { "Overwrote" } else { "Created" },
        content.len()
    ))
}

fn edit_file(args: Value) -> Result<String, String> {
    let path = args["path"].as_str().ok_or("path is required")?;
    let old_string = args["old_string"].as_str().ok_or("old_string is required")?;
    let new_string = args["new_string"].as_str().ok_or("new_string is required")?;

    if old_string.is_empty() {
        return Err("old_string cannot be empty".to_string());
    }

    let resolved = resolve_path(path)?;
    check_file_size(&resolved)?;

    let content =
        fs::read_to_string(&resolved).map_err(|e| format!("Cannot read {path}: {e}"))?;

    let count = content.matches(old_string).count();
    if count == 0 {
        return Err(format!("old_string not found in {path}"));
    }
    if count > 1 {
        return Err(format!(
            "old_string found {count} times in {path}. Provide more context to make it unique."
        ));
    }

    let new_content = content.replacen(old_string, new_string, 1);
    fs::write(&resolved, &new_content).map_err(|e| format!("Cannot write {path}: {e}"))?;

    let old_lines = old_string.lines().count();
    let new_lines = new_string.lines().count();
    Ok(format!(
        "Edited {path}: replaced {old_lines} line(s) with {new_lines} line(s)"
    ))
}
