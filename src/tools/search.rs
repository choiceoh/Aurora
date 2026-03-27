use super::Registry;
use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub fn register(reg: &mut Registry) {
    reg.register_tool(
        "grep",
        "Search file contents using a regex pattern. Returns matching lines with paths and line numbers.",
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regex pattern to search for" },
                "path": { "type": "string", "description": "Directory/file to search (default: current dir)" },
                "include": { "type": "string", "description": "Glob to filter files (e.g. '*.rs')" }
            },
            "required": ["pattern"]
        }),
        Box::new(grep_search),
    );

    reg.register_tool(
        "glob",
        "Find files matching a glob pattern.",
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern (e.g. '**/*.rs')" },
                "path": { "type": "string", "description": "Base directory (default: current dir)" }
            },
            "required": ["pattern"]
        }),
        Box::new(glob_search),
    );
}

const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", "vendor", "__pycache__"];
const BINARY_EXTS: &[&str] = &[
    ".exe", ".bin", ".so", ".dll", ".dylib", ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".ico",
    ".zip", ".tar", ".gz", ".bz2", ".7z", ".pdf", ".mp3", ".mp4", ".wasm", ".o", ".a",
];
const MAX_MATCHES: usize = 200;

fn grep_search(args: Value) -> Result<String, String> {
    let pattern = args["pattern"].as_str().ok_or("pattern required")?;
    let search_path = args["path"].as_str().unwrap_or(".");
    let include = args["include"].as_str();

    let re = Regex::new(pattern).map_err(|e| format!("Invalid regex: {e}"))?;

    let base = if Path::new(search_path).is_absolute() {
        search_path.to_string()
    } else {
        std::env::current_dir()
            .unwrap_or_default()
            .join(search_path)
            .to_string_lossy()
            .to_string()
    };

    let mut results = Vec::new();
    walk_grep(&base, &base, &re, include, &mut results);

    if results.is_empty() {
        return Ok("No matches found.".to_string());
    }

    let mut output = results.join("\n");
    if results.len() >= MAX_MATCHES {
        output.push_str(&format!("\n[Results truncated at {MAX_MATCHES} matches]"));
    }
    Ok(output)
}

fn walk_grep(
    base: &str,
    dir: &str,
    re: &Regex,
    include: Option<&str>,
    results: &mut Vec<String>,
) {
    if results.len() >= MAX_MATCHES {
        return;
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if results.len() >= MAX_MATCHES {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if path.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk_grep(base, &path.to_string_lossy(), re, include, results);
        } else {
            if is_binary(&name) {
                continue;
            }
            if let Some(pattern) = include {
                if !glob_match_name(pattern, &name) {
                    continue;
                }
            }

            if let Ok(content) = fs::read_to_string(&path) {
                let rel = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy();
                for (i, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        results.push(format!("{}:{}: {}", rel, i + 1, line));
                        if results.len() >= MAX_MATCHES {
                            return;
                        }
                    }
                }
            }
        }
    }
}

fn glob_search(args: Value) -> Result<String, String> {
    let pattern = args["pattern"].as_str().ok_or("pattern required")?;
    let base_path = args["path"].as_str().unwrap_or(".");

    let base = if Path::new(base_path).is_absolute() {
        base_path.to_string()
    } else {
        std::env::current_dir()
            .unwrap_or_default()
            .join(base_path)
            .to_string_lossy()
            .to_string()
    };

    let full_pattern = format!("{}/{}", base, pattern);
    let matches: Vec<String> = glob::glob(&full_pattern)
        .map_err(|e| format!("Invalid glob: {e}"))?
        .filter_map(|entry| entry.ok())
        .filter(|p| {
            !p.components().any(|c| {
                SKIP_DIRS.contains(&c.as_os_str().to_string_lossy().as_ref())
            })
        })
        .take(500)
        .filter_map(|p| {
            p.strip_prefix(&base)
                .ok()
                .map(|r| r.to_string_lossy().to_string())
        })
        .collect();

    if matches.is_empty() {
        return Ok("No matching files found.".to_string());
    }

    let mut result = matches.join("\n");
    if matches.len() >= 500 {
        result.push_str("\n[Results truncated at 500 files]");
    }
    Ok(result)
}

fn is_binary(name: &str) -> bool {
    let lower = name.to_lowercase();
    BINARY_EXTS.iter().any(|ext| lower.ends_with(ext))
}

fn glob_match_name(pattern: &str, name: &str) -> bool {
    glob::Pattern::new(pattern)
        .map(|p| p.matches(name))
        .unwrap_or(false)
}
