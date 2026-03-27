//! Native ripgrep integration using the grep crate directly.
//!
//! Replaces subprocess-based `rg` invocation with direct use of ripgrep's
//! grep-searcher, grep-regex, and grep-matcher crates. Benefits:
//! - No fork+exec overhead per search call
//! - Shared compiled regex matchers
//! - Memory-mapped file I/O where available
//! - SIMD-accelerated matching via the regex engine

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use serde_json::{json, Value};
use std::path::Path;

use super::Registry;

const MAX_MATCHES: usize = 200;
const MAX_FILE_MATCHES: usize = 500;

const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", "vendor", "__pycache__", ".hg", ".svn", "dist", "build"];
const BINARY_EXTS: &[&str] = &[
    ".exe", ".bin", ".so", ".dll", ".dylib", ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".ico",
    ".zip", ".tar", ".gz", ".bz2", ".7z", ".pdf", ".mp3", ".mp4", ".wasm", ".o", ".a",
    ".class", ".pyc", ".pyo",
];

pub fn register(reg: &mut Registry) {
    reg.register_tool(
        "grep",
        "Search file contents using a regex pattern. Uses native ripgrep engine for fast, \
         SIMD-accelerated matching. Returns matching lines with paths and line numbers.",
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regex pattern to search for" },
                "path": { "type": "string", "description": "Directory/file to search (default: current dir)" },
                "include": { "type": "string", "description": "Glob to filter files (e.g. '*.rs')" },
                "case_insensitive": { "type": "boolean", "description": "Case insensitive search (default: false)" },
                "context_lines": { "type": "integer", "description": "Number of context lines before/after match (default: 0)" }
            },
            "required": ["pattern"]
        }),
        Box::new(native_grep),
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

fn native_grep(args: Value) -> Result<String, String> {
    let pattern = args["pattern"].as_str().ok_or("pattern required")?;
    let search_path = args["path"].as_str().unwrap_or(".");
    let include = args["include"].as_str();
    let case_insensitive = args["case_insensitive"].as_bool().unwrap_or(false);
    let context_lines = args["context_lines"].as_i64().unwrap_or(0).max(0) as usize;

    // Build the regex matcher using ripgrep's engine (DFA-based, SIMD-accelerated)
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive)
        .multi_line(false)
        .build(pattern)
        .map_err(|e| format!("Invalid regex: {e}"))?;

    // Build a searcher config (reused to create searchers without clone overhead)
    let mut searcher_builder = SearcherBuilder::new();
    searcher_builder
        .binary_detection(BinaryDetection::quit(0x00))
        .line_number(true);

    if context_lines > 0 {
        searcher_builder
            .before_context(context_lines)
            .after_context(context_lines);
    }

    let base = resolve_path(search_path)?;
    let mut results = Vec::new();

    if Path::new(&base).is_file() {
        search_file(&base, &base, &matcher, &searcher_builder, &mut results);
    } else {
        walk_and_search(&base, &base, &matcher, &searcher_builder, include, &mut results);
    }

    if results.is_empty() {
        return Ok("No matches found.".to_string());
    }

    let mut output = results.join("\n");
    if results.len() >= MAX_MATCHES {
        output.push_str(&format!("\n[Results truncated at {MAX_MATCHES} matches]"));
    }
    Ok(output)
}

fn search_file(
    file_path: &str,
    base: &str,
    matcher: &grep_regex::RegexMatcher,
    searcher_builder: &SearcherBuilder,
    results: &mut Vec<String>,
) {
    let mut searcher = searcher_builder.build();
    let rel = Path::new(file_path)
        .strip_prefix(base)
        .unwrap_or(Path::new(file_path))
        .to_string_lossy()
        .to_string();

    let rel_display = if rel.is_empty() {
        Path::new(file_path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| file_path.to_string())
    } else {
        rel
    };

    let _ = searcher.search_path(
        matcher,
        Path::new(file_path),
        UTF8(|line_num, line| {
            if results.len() >= MAX_MATCHES {
                return Ok(false); // Stop searching
            }
            let line = line.trim_end_matches('\n').trim_end_matches('\r');
            results.push(format!("{}:{}: {}", rel_display, line_num, line));
            Ok(true)
        }),
    );
}

fn walk_and_search(
    dir: &str,
    base: &str,
    matcher: &grep_regex::RegexMatcher,
    searcher_builder: &SearcherBuilder,
    include: Option<&str>,
    results: &mut Vec<String>,
) {
    if results.len() >= MAX_MATCHES {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
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
            walk_and_search(&path.to_string_lossy(), base, matcher, searcher_builder, include, results);
        } else {
            if is_binary(&name) {
                continue;
            }
            if let Some(pattern) = include {
                if !glob_match_name(pattern, &name) {
                    continue;
                }
            }
            search_file(&path.to_string_lossy(), base, matcher, searcher_builder, results);
        }
    }
}

fn glob_search(args: Value) -> Result<String, String> {
    let pattern = args["pattern"].as_str().ok_or("pattern required")?;
    let base_path = args["path"].as_str().unwrap_or(".");

    let base = resolve_path(base_path)?;
    let full_pattern = std::path::Path::new(&base)
        .join(pattern)
        .to_string_lossy()
        .to_string();

    let matches: Vec<String> = glob::glob(&full_pattern)
        .map_err(|e| format!("Invalid glob: {e}"))?
        .filter_map(|entry| entry.ok())
        .filter(|p| {
            !p.components().any(|c| {
                SKIP_DIRS.contains(&c.as_os_str().to_string_lossy().as_ref())
            })
        })
        .take(MAX_FILE_MATCHES)
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
    if matches.len() >= MAX_FILE_MATCHES {
        result.push_str(&format!("\n[Results truncated at {MAX_FILE_MATCHES} files]"));
    }
    Ok(result)
}

fn resolve_path(path: &str) -> Result<String, String> {
    let p = Path::new(path);
    if p.is_absolute() {
        Ok(path.to_string())
    } else {
        Ok(std::env::current_dir()
            .map_err(|e| format!("Cannot get working directory: {e}"))?
            .join(path)
            .to_string_lossy()
            .to_string())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_grep_basic() {
        // Search in the current project for a known pattern
        let args = json!({
            "pattern": "fn register",
            "path": ".",
            "include": "*.rs"
        });
        let result = native_grep(args).unwrap();
        assert!(result.contains("fn register"), "Should find function definitions");
    }

    #[test]
    fn test_native_grep_case_insensitive() {
        let args = json!({
            "pattern": "FN REGISTER",
            "path": ".",
            "include": "*.rs",
            "case_insensitive": true
        });
        let result = native_grep(args).unwrap();
        assert!(result.contains("fn register"), "Case insensitive should match");
    }

    #[test]
    fn test_native_grep_no_match() {
        // Search only in Cargo.toml to avoid matching this test file itself
        let args = json!({
            "pattern": "xyzzy_nonexistent_pattern_12345",
            "path": "Cargo.toml"
        });
        let result = native_grep(args).unwrap();
        assert_eq!(result, "No matches found.");
    }

    #[test]
    fn test_native_grep_invalid_regex() {
        let args = json!({
            "pattern": "[invalid",
            "path": "."
        });
        let result = native_grep(args);
        assert!(result.is_err(), "Invalid regex should return error");
    }

    #[test]
    fn test_glob_search() {
        let args = json!({
            "pattern": "**/*.rs",
            "path": "."
        });
        let result = glob_search(args).unwrap();
        assert!(result.contains(".rs"), "Should find Rust files");
    }
}
