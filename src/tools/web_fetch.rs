use super::Registry;
use crate::preprocessing;
use serde_json::{json, Value};

const MAX_HTML_SIZE: usize = 5 * 1024 * 1024; // 5MB
const MAX_OUTPUT_CHARS: usize = 50_000;
const DEFAULT_TIMEOUT_SECS: u64 = 30;

pub fn register(reg: &mut Registry) {
    reg.register_tool(
        "web_fetch",
        "Fetch a web page and return its cleaned content with metadata. Strips noise (nav, ads, \
         cookie banners, etc.), extracts metadata (title, OG tags, JSON-LD), and detects quality \
         issues (paywall, login wall, bot protection).",
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
                "raw": {
                    "type": "boolean",
                    "description": "Return raw preprocessed JSON instead of formatted text (default: false)"
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
    let raw = args["raw"].as_bool().unwrap_or(false);

    // Validate URL
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("URL must start with http:// or https://".to_string());
    }

    // Fetch HTML synchronously using blocking reqwest
    let html = fetch_url(url)?;

    // Run preprocessing pipeline
    let result = preprocessing::preprocess_html(&html);

    if raw {
        return serde_json::to_string_pretty(&result)
            .map_err(|e| format!("Serialization error: {e}"));
    }

    // Format output
    let mut output = if include_metadata {
        preprocessing::html_preprocess::format_result(&result)
    } else {
        result.cleaned.clone()
    };

    // Truncate if too long
    if output.len() > MAX_OUTPUT_CHARS {
        let truncated: String = output.chars().take(MAX_OUTPUT_CHARS).collect();
        output = format!(
            "{truncated}\n\n[Truncated: showing {MAX_OUTPUT_CHARS}/{} chars]",
            output.len()
        );
    }

    Ok(output)
}

fn fetch_url(url: &str) -> Result<String, String> {
    // Use a blocking HTTP client since tools run synchronously
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
