use regex::Regex;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// Safely slice a string up to `max` bytes, never panicking on multi-byte boundaries.
fn safe_prefix(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ─── Compiled regex patterns (compiled once, reused forever) ───

macro_rules! lazy_regex {
    ($pattern:expr) => {
        LazyLock::new(|| Regex::new($pattern).expect(concat!("Invalid regex: ", $pattern)))
    };
}

// Noise element patterns (matched against tag+class+id)
static RE_NAV: LazyLock<Regex> = lazy_regex!(r"(?i)(nav(bar|igation)?|main[-_]?menu|sidebar|side-bar)");
static RE_FOOTER: LazyLock<Regex> = lazy_regex!(r"(?i)(footer|foot|bottom-bar)");
static RE_AD: LazyLock<Regex> =
    lazy_regex!(r"(?i)(ad[-_]?banner|advert|ads[-_]?container|sponsor|promo[-_]?box|google[-_]?ad)");
static RE_COOKIE: LazyLock<Regex> =
    lazy_regex!(r"(?i)(cookie[-_]?(banner|consent|notice|popup)|gdpr|consent[-_]?(bar|modal))");
static RE_SOCIAL: LazyLock<Regex> =
    lazy_regex!(r"(?i)(social[-_]?(share|links|buttons|widget)|share[-_]?(bar|buttons))");
static RE_POPUP: LazyLock<Regex> =
    lazy_regex!(r"(?i)(popup|modal[-_]?(dialog|window|backdrop)|lightbox|newsletter[-_]?(signup|modal))");
static RE_COMMENT_SECTION: LazyLock<Regex> =
    lazy_regex!(r"(?i)(comments?[-_]?(section|area|list|block)|disqus)");
static RE_RELATED: LazyLock<Regex> =
    lazy_regex!(r"(?i)(related[-_]?(posts|articles|content)|recommended|you[-_]?might[-_]?like)");

// Quality signal patterns
static RE_PAYWALL: LazyLock<Regex> =
    lazy_regex!(r"(?i)(paywall|subscribe[-_]?to[-_]?(read|continue|access)|premium[-_]?content|membership[-_]?required)");
static RE_LOGIN_WALL: LazyLock<Regex> =
    lazy_regex!(r"(?i)(login[-_]?(to[-_]?continue|required|wall)|sign[-_]?in[-_]?to[-_]?(view|continue|access)|auth[-_]?gate)");
static RE_BOT_BLOCK: LazyLock<Regex> =
    lazy_regex!(r"(?i)(captcha|recaptcha|hcaptcha|cloudflare[-_]?challenge|bot[-_]?(check|detection|protection)|access[-_]?denied)");
static RE_SPA_SHELL: LazyLock<Regex> =
    lazy_regex!(r"(?i)(noscript|enable[-_]?javascript|javascript[-_]?required|app[-_]?root|__next|__nuxt|react[-_]?root)");

// Whitespace normalization
static RE_MULTI_NEWLINE: LazyLock<Regex> = lazy_regex!(r"\n{3,}");
static RE_MULTI_SPACE: LazyLock<Regex> = lazy_regex!(r"[ \t]{2,}");

// ─── Data structures ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreprocessResult {
    /// Cleaned plain text content with noise elements removed
    pub cleaned: String,
    /// Cleaned HTML with noise elements removed (for downstream markdown conversion)
    pub cleaned_html: String,
    /// Extracted metadata
    pub metadata: HtmlMetadata,
    /// Quality signals detected in the page
    pub signals: QualitySignals,
    /// Processing statistics
    pub stats: ProcessingStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HtmlMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub og_title: Option<String>,
    pub og_description: Option<String>,
    pub og_image: Option<String>,
    pub og_type: Option<String>,
    pub og_url: Option<String>,
    pub canonical_url: Option<String>,
    pub charset: Option<String>,
    pub language: Option<String>,
    pub author: Option<String>,
    pub published_date: Option<String>,
    pub json_ld: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QualitySignals {
    pub has_paywall: bool,
    pub has_login_wall: bool,
    pub has_bot_block: bool,
    pub is_spa_shell: bool,
    /// Approximate ratio of content vs boilerplate (0.0 - 1.0)
    pub content_ratio: f32,
    /// Whether the page seems to have meaningful content
    pub has_content: bool,
    /// Detected issues
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessingStats {
    pub original_len: usize,
    pub cleaned_len: usize,
    pub elements_removed: usize,
}

// ─── Noise element tags to remove entirely ───

const NOISE_TAGS: &[&str] = &[
    "script", "style", "noscript", "iframe", "svg", "canvas",
    "template", "picture", "source", "video", "audio",
];

// Selectors for structural noise elements
const NOISE_SELECTORS: &[&str] = &[
    "nav",
    "aside",
    "header nav",
    "[role=\"navigation\"]",
    "[role=\"banner\"]",
    "[role=\"complementary\"]",
    "[aria-hidden=\"true\"]",
];

/// Preprocess raw HTML: strip noise, extract metadata, detect quality signals.
///
/// This performs all three operations in a single pass over the parsed DOM,
/// avoiding the overhead of multiple regex passes over raw HTML text.
pub fn preprocess_html(raw_html: &str) -> PreprocessResult {
    let original_len = raw_html.len();
    let document = Html::parse_document(raw_html);

    // Phase 1: Extract metadata from <head> (fast, selector-based)
    let metadata = extract_metadata(&document, raw_html);

    // Phase 2: Detect quality signals (checks both DOM and raw text)
    let signals = detect_quality_signals(&document, raw_html);

    // Phase 3: Clean HTML by removing noise elements, return text + cleaned HTML
    let (cleaned, cleaned_html, elements_removed) = strip_noise(&document);

    let cleaned_len = cleaned.len();

    PreprocessResult {
        cleaned,
        cleaned_html,
        metadata,
        signals,
        stats: ProcessingStats {
            original_len,
            cleaned_len,
            elements_removed,
        },
    }
}

/// Extract metadata from HTML head: title, OG tags, JSON-LD, charset, etc.
fn extract_metadata(document: &Html, raw_html: &str) -> HtmlMetadata {
    let mut meta = HtmlMetadata::default();

    // Title
    if let Some(sel) = Selector::parse("title").ok() {
        if let Some(el) = document.select(&sel).next() {
            let text = el.text().collect::<String>().trim().to_string();
            if !text.is_empty() {
                meta.title = Some(text);
            }
        }
    }

    // Meta tags
    if let Some(sel) = Selector::parse("meta").ok() {
        for el in document.select(&sel) {
            let name = el.attr("name").or_else(|| el.attr("property")).unwrap_or("");
            let content = el.attr("content").unwrap_or("");
            let charset_attr = el.attr("charset");

            if let Some(cs) = charset_attr {
                meta.charset = Some(cs.to_string());
                continue;
            }

            if content.is_empty() {
                // Check http-equiv for charset
                if let Some(http_equiv) = el.attr("http-equiv") {
                    if http_equiv.eq_ignore_ascii_case("content-type") {
                        if let Some(ct) = el.attr("content") {
                            if let Some(pos) = ct.to_lowercase().find("charset=") {
                                meta.charset = Some(ct[pos + 8..].trim().to_string());
                            }
                        }
                    }
                }
                continue;
            }

            match name.to_lowercase().as_str() {
                "description" => meta.description = Some(content.to_string()),
                "author" => meta.author = Some(content.to_string()),
                "og:title" => meta.og_title = Some(content.to_string()),
                "og:description" => meta.og_description = Some(content.to_string()),
                "og:image" => meta.og_image = Some(content.to_string()),
                "og:type" => meta.og_type = Some(content.to_string()),
                "og:url" => meta.og_url = Some(content.to_string()),
                "article:published_time" | "date" | "publisheddate" => {
                    meta.published_date = Some(content.to_string());
                }
                _ => {}
            }
        }
    }

    // Canonical URL
    if let Some(sel) = Selector::parse("link[rel=\"canonical\"]").ok() {
        if let Some(el) = document.select(&sel).next() {
            if let Some(href) = el.attr("href") {
                meta.canonical_url = Some(href.to_string());
            }
        }
    }

    // Language from <html lang="...">
    if let Some(sel) = Selector::parse("html").ok() {
        if let Some(el) = document.select(&sel).next() {
            if let Some(lang) = el.attr("lang") {
                meta.language = Some(lang.to_string());
            }
        }
    }

    // JSON-LD structured data
    if let Some(sel) = Selector::parse("script[type=\"application/ld+json\"]").ok() {
        for el in document.select(&sel) {
            let text = el.text().collect::<String>();
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(text.trim()) {
                meta.json_ld.push(json);
            }
        }
    }

    // Charset fallback: detect from raw HTML if not found in meta
    if meta.charset.is_none() {
        let head_portion = safe_prefix(raw_html, 2048);
        let lower = head_portion.to_lowercase();
        if let Some(pos) = lower.find("charset=") {
            let after = &head_portion[pos + 8..];
            let charset: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if !charset.is_empty() {
                meta.charset = Some(charset);
            }
        }
    }

    meta
}

/// Detect quality signals: paywall, login wall, bot block, SPA shell.
fn detect_quality_signals(document: &Html, raw_html: &str) -> QualitySignals {
    let mut signals = QualitySignals::default();

    // Check raw HTML text for signal patterns (single pass over text)
    let check_text = safe_prefix(raw_html, 50_000);

    if RE_PAYWALL.is_match(check_text) {
        signals.has_paywall = true;
        signals.issues.push("Paywall detected".to_string());
    }

    if RE_LOGIN_WALL.is_match(check_text) {
        signals.has_login_wall = true;
        signals.issues.push("Login wall detected".to_string());
    }

    if RE_BOT_BLOCK.is_match(check_text) {
        signals.has_bot_block = true;
        signals.issues.push("Bot protection detected".to_string());
    }

    // SPA shell detection: check for empty body with JS framework markers
    if RE_SPA_SHELL.is_match(check_text) {
        // Confirm by checking if body has very little text content
        if let Some(sel) = Selector::parse("body").ok() {
            if let Some(body) = document.select(&sel).next() {
                let body_text: String = body.text().collect();
                let text_len = body_text.trim().len();
                if text_len < 200 {
                    signals.is_spa_shell = true;
                    signals.issues.push("SPA shell detected (minimal content)".to_string());
                }
            }
        }
    }

    // Content ratio estimation
    let total_len = raw_html.len();
    if let Some(sel) = Selector::parse("body").ok() {
        if let Some(body) = document.select(&sel).next() {
            let text: String = body.text().collect();
            let text_len = text.trim().len();
            signals.content_ratio = if total_len > 0 {
                (text_len as f32 / total_len as f32).min(1.0)
            } else {
                0.0
            };
            signals.has_content = text_len > 100;
        }
    }

    signals
}

/// Strip noise elements from HTML document.
/// Returns (cleaned_text, cleaned_html, elements_removed).
fn strip_noise(document: &Html) -> (String, String, usize) {
    // Collect IDs of elements to skip (using HashSet for dedup)
    let mut skip_ids = std::collections::HashSet::new();

    // Mark noise tags for removal
    for tag_name in NOISE_TAGS {
        if let Ok(sel) = Selector::parse(tag_name) {
            for el in document.select(&sel) {
                skip_ids.insert(el.id());
            }
        }
    }

    // Mark structural noise selectors
    for selector_str in NOISE_SELECTORS {
        if let Ok(sel) = Selector::parse(selector_str) {
            for el in document.select(&sel) {
                skip_ids.insert(el.id());
            }
        }
    }

    // Mark class/id-based noise elements by checking against regex patterns
    let noise_patterns: &[&LazyLock<Regex>] = &[
        &RE_NAV,
        &RE_FOOTER,
        &RE_AD,
        &RE_COOKIE,
        &RE_SOCIAL,
        &RE_POPUP,
        &RE_COMMENT_SECTION,
        &RE_RELATED,
    ];

    if let Ok(sel) = Selector::parse("[class], [id]") {
        for el in document.select(&sel) {
            let class = el.attr("class").unwrap_or("");
            let id = el.attr("id").unwrap_or("");
            let combined = format!("{class} {id}");

            for pattern in noise_patterns {
                if pattern.is_match(&combined) {
                    skip_ids.insert(el.id());
                    break;
                }
            }
        }
    }

    let removed = skip_ids.len();

    // Extract plain text from body, skipping marked elements
    let cleaned = extract_text_excluding(document, &skip_ids);

    // Rebuild HTML with noise elements removed
    let cleaned_html = rebuild_html_excluding(document, &skip_ids);

    // Normalize whitespace in plain text
    let cleaned = RE_MULTI_NEWLINE.replace_all(&cleaned, "\n\n").to_string();
    let cleaned = RE_MULTI_SPACE.replace_all(&cleaned, " ").to_string();
    let cleaned = cleaned.trim().to_string();

    (cleaned, cleaned_html, removed)
}

/// Rebuild HTML string from the document, excluding noise elements.
/// This produces minimal HTML suitable for html2md conversion.
fn rebuild_html_excluding(
    document: &Html,
    skip_ids: &std::collections::HashSet<ego_tree::NodeId>,
) -> String {
    use scraper::node::Node;

    let mut output = String::new();

    let body_sel = Selector::parse("body").unwrap();
    let body = match document.select(&body_sel).next() {
        Some(b) => b,
        None => return output,
    };

    fn walk_html(
        node_ref: ego_tree::NodeRef<'_, Node>,
        skip_ids: &std::collections::HashSet<ego_tree::NodeId>,
        output: &mut String,
    ) {
        if skip_ids.contains(&node_ref.id()) {
            return;
        }

        match node_ref.value() {
            Node::Text(text) => {
                // Escape HTML entities in text nodes
                let t = &text.text;
                for ch in t.chars() {
                    match ch {
                        '&' => output.push_str("&amp;"),
                        '<' => output.push_str("&lt;"),
                        '>' => output.push_str("&gt;"),
                        _ => output.push(ch),
                    }
                }
            }
            Node::Element(el) => {
                let tag = el.name();
                output.push('<');
                output.push_str(tag);
                for (key, val) in el.attrs() {
                    output.push(' ');
                    output.push_str(key);
                    output.push_str("=\"");
                    output.push_str(val);
                    output.push('"');
                }
                output.push('>');

                for child in node_ref.children() {
                    walk_html(child, skip_ids, output);
                }

                // Close tag (skip void elements)
                if !matches!(tag, "br" | "hr" | "img" | "input" | "meta" | "link" | "source" | "col" | "area" | "base" | "embed" | "wbr") {
                    output.push_str("</");
                    output.push_str(tag);
                    output.push('>');
                }
            }
            _ => {
                for child in node_ref.children() {
                    walk_html(child, skip_ids, output);
                }
            }
        }
    }

    for child in body.children() {
        walk_html(child, skip_ids, &mut output);
    }

    output
}

/// Extract text content from the document body, excluding elements with given IDs.
fn extract_text_excluding(
    document: &Html,
    skip_ids: &std::collections::HashSet<ego_tree::NodeId>,
) -> String {
    use scraper::node::Node;

    let mut output = String::new();

    let body_sel = Selector::parse("body").unwrap();
    let body = match document.select(&body_sel).next() {
        Some(b) => b,
        None => return output,
    };

    // Walk the DOM tree under <body>
    fn walk(
        node_ref: ego_tree::NodeRef<'_, Node>,
        skip_ids: &std::collections::HashSet<ego_tree::NodeId>,
        output: &mut String,
    ) {
        let node_id = node_ref.id();

        // Check if this node should be skipped
        if skip_ids.contains(&node_id) {
            return;
        }

        match node_ref.value() {
            Node::Text(text) => {
                let t = text.text.trim();
                if !t.is_empty() {
                    output.push_str(t);
                    output.push(' ');
                }
            }
            Node::Element(el) => {
                // Add line breaks for block-level elements
                let tag = el.name();
                let is_block = matches!(
                    tag,
                    "p" | "div"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "li"
                        | "tr"
                        | "br"
                        | "hr"
                        | "blockquote"
                        | "pre"
                        | "article"
                        | "section"
                        | "main"
                        | "figure"
                        | "figcaption"
                        | "dt"
                        | "dd"
                );

                if is_block && !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }

                // Add heading markers
                match tag {
                    "h1" => output.push_str("# "),
                    "h2" => output.push_str("## "),
                    "h3" => output.push_str("### "),
                    "h4" => output.push_str("#### "),
                    "h5" => output.push_str("##### "),
                    "h6" => output.push_str("###### "),
                    "li" => output.push_str("- "),
                    _ => {}
                }

                for child in node_ref.children() {
                    walk(child, skip_ids, output);
                }

                if is_block {
                    output.push('\n');
                }
            }
            _ => {
                for child in node_ref.children() {
                    walk(child, skip_ids, output);
                }
            }
        }
    }

    for child in body.children() {
        walk(child, skip_ids, &mut output);
    }

    output
}

/// Format the preprocessing result as a concise summary string.
pub fn format_result(result: &PreprocessResult) -> String {
    let mut out = String::new();

    // Metadata header
    if let Some(ref title) = result.metadata.title {
        out.push_str(&format!("Title: {title}\n"));
    }
    if let Some(ref desc) = result.metadata.description.as_ref().or(result.metadata.og_description.as_ref()) {
        out.push_str(&format!("Description: {desc}\n"));
    }
    if let Some(ref author) = result.metadata.author {
        out.push_str(&format!("Author: {author}\n"));
    }
    if let Some(ref date) = result.metadata.published_date {
        out.push_str(&format!("Published: {date}\n"));
    }
    if let Some(ref url) = result.metadata.canonical_url.as_ref().or(result.metadata.og_url.as_ref()) {
        out.push_str(&format!("URL: {url}\n"));
    }

    // Quality warnings
    if !result.signals.issues.is_empty() {
        out.push_str(&format!("\nWarnings: {}\n", result.signals.issues.join(", ")));
    }

    // Stats
    out.push_str(&format!(
        "\n[Preprocessed: {} → {} bytes, {} noise elements removed, {:.0}% content ratio]\n",
        result.stats.original_len,
        result.stats.cleaned_len,
        result.stats.elements_removed,
        result.signals.content_ratio * 100.0,
    ));

    // Content separator
    out.push_str("\n---\n\n");
    out.push_str(&result.cleaned);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_preprocessing() {
        let html = r#"
        <html lang="en">
        <head>
            <title>Test Page</title>
            <meta name="description" content="A test page">
            <meta property="og:title" content="OG Test">
            <meta charset="utf-8">
        </head>
        <body>
            <nav class="main-nav"><a href="/">Home</a></nav>
            <main>
                <h1>Hello World</h1>
                <p>This is the main content of the page.</p>
            </main>
            <footer>Copyright 2024</footer>
            <script>console.log("noise");</script>
        </body>
        </html>"#;

        let result = preprocess_html(html);

        assert_eq!(result.metadata.title.as_deref(), Some("Test Page"));
        assert_eq!(result.metadata.description.as_deref(), Some("A test page"));
        assert_eq!(result.metadata.og_title.as_deref(), Some("OG Test"));
        assert_eq!(result.metadata.charset.as_deref(), Some("utf-8"));
        assert_eq!(result.metadata.language.as_deref(), Some("en"));

        assert!(result.cleaned.contains("Hello World"));
        assert!(result.cleaned.contains("main content"));
        assert!(!result.cleaned.contains("console.log"));
        assert!(result.stats.elements_removed > 0);
    }

    #[test]
    fn test_quality_signals() {
        let html = r#"
        <html>
        <body>
            <div class="paywall-overlay">Subscribe to continue reading</div>
            <div class="captcha-container">Please verify you're not a bot</div>
            <p>Some content</p>
        </body>
        </html>"#;

        let result = preprocess_html(html);
        assert!(result.signals.has_paywall);
        assert!(result.signals.has_bot_block);
    }

    #[test]
    fn test_json_ld_extraction() {
        let html = r#"
        <html>
        <head>
            <script type="application/ld+json">
            {"@type": "Article", "headline": "Test Article"}
            </script>
        </head>
        <body><p>Content</p></body>
        </html>"#;

        let result = preprocess_html(html);
        assert_eq!(result.metadata.json_ld.len(), 1);
        assert_eq!(result.metadata.json_ld[0]["headline"], "Test Article");
    }

    #[test]
    fn test_noise_removal() {
        let html = r#"
        <html>
        <body>
            <div class="cookie-banner">Accept cookies</div>
            <div class="ad-banner">Buy stuff!</div>
            <div class="social-share">Share on Twitter</div>
            <article>
                <h1>Real Article</h1>
                <p>Important content here.</p>
            </article>
            <div class="related-posts">You might also like...</div>
        </body>
        </html>"#;

        let result = preprocess_html(html);
        assert!(result.cleaned.contains("Real Article"));
        assert!(result.cleaned.contains("Important content"));
        assert!(!result.cleaned.contains("Accept cookies"));
        assert!(!result.cleaned.contains("Buy stuff"));
        // Related posts div should be removed
        assert!(result.stats.elements_removed >= 3);
    }

    #[test]
    fn test_empty_html() {
        let result = preprocess_html("");
        assert!(result.cleaned.is_empty() || result.cleaned.trim().is_empty());
        assert!(!result.signals.has_content);
    }
}
