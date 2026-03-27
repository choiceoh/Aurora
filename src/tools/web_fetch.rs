use super::Registry;
use crate::preprocessing::{self, WebContentConfig};
use serde_json::{json, Value};

const MAX_HTML_SIZE: usize = 5 * 1024 * 1024; // 5MB
const DEFAULT_TIMEOUT_SECS: u64 = 30;

pub fn register(reg: &mut Registry) {
    reg.register_tool(
        "web_fetch",
        "Fetch a web page and return its cleaned Markdown content with metadata. \
         Single-pass pipeline: noise strip → Markdown conversion → section-based truncation. \
         Extracts metadata (title, OG tags, JSON-LD) and detects quality issues.",
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to fetch"
                },
                "include_metadata": {
                    "type": "boolean",
                    "description": "Include metadata header in output (default: true)"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum output characters (default: 50000)"
                },
                "raw": {
                    "type": "boolean",
                    "description": "Return raw JSON with metadata/signals/content (default: false)"
                }
            },
            "required": ["url"]
        }),
        Box::new(web_fetch),
    );
}

fn web_fetch(args: Value) -> Result<String, String> {
    let url = args["url"].as_str().ok_or("url is required")?;
    let include_metadata = args["include_metadata"].as_bool().unwrap_or(true);
    let max_chars = args["max_chars"].as_u64().unwrap_or(50_000) as usize;
    let raw = args["raw"].as_bool().unwrap_or(false);

    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("URL must start with http:// or https://".to_string());
    }

    // Fetch HTML
    let html = fetch_url(url)?;

    if raw {
        // Raw mode: return preprocessing result as JSON (no markdown conversion)
        let result = preprocessing::preprocess_html(&html);
        return serde_json::to_string_pretty(&result)
            .map_err(|e| format!("Serialization error: {e}"));
    }

    // Unified pipeline: preprocess → markdown → truncate → format (single call)
    let config = WebContentConfig {
        max_chars,
        include_metadata,
        include_warnings: true,
        to_markdown: true,
    };

    let result = preprocessing::process_web_content(&html, &config);
    Ok(result.content)
}

fn fetch_url(url: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("Mozilla/5.0 (compatible; Aurora/0.1; +https://github.com/choiceoh/Aurora)")
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let response = client
        .get(url)
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.5,ko;q=0.3")
        .send()
        .map_err(|e| format!("Fetch error: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {status} for {url}"));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !content_type.is_empty()
        && !content_type.contains("text/html")
        && !content_type.contains("application/xhtml")
        && !content_type.contains("text/plain")
    {
        return Err(format!("Non-HTML content type: {content_type}"));
    }

    let bytes = response
        .bytes()
        .map_err(|e| format!("Read error: {e}"))?;

    if bytes.len() > MAX_HTML_SIZE {
        return Err(format!(
            "Response too large: {:.1}MB > {:.0}MB limit",
            bytes.len() as f64 / 1_048_576.0,
            MAX_HTML_SIZE as f64 / 1_048_576.0
        ));
    }

    Ok(String::from_utf8_lossy(&bytes).to_string())
}
