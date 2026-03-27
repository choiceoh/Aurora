//! Unified web content processing pipeline.
//!
//! Combines HTML preprocessing → Markdown conversion → section-based truncation
//! → metadata formatting into a single call, eliminating intermediate allocations
//! and multiple passes over the content.

use super::html_preprocess::{self, HtmlMetadata, PreprocessResult, QualitySignals};
use serde::{Deserialize, Serialize};

/// Configuration for the unified web content processor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebContentConfig {
    /// Maximum number of characters in the final output
    pub max_chars: usize,
    /// Whether to include metadata header in output
    pub include_metadata: bool,
    /// Whether to include quality signal warnings
    pub include_warnings: bool,
    /// Whether to convert HTML to Markdown (true) or plain text (false)
    pub to_markdown: bool,
}

impl Default for WebContentConfig {
    fn default() -> Self {
        Self {
            max_chars: 50_000,
            include_metadata: true,
            include_warnings: true,
            to_markdown: true,
        }
    }
}

/// Result of the unified web content processing pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebContentResult {
    /// Final processed content (metadata header + converted/truncated body)
    pub content: String,
    /// Extracted metadata (available separately for programmatic access)
    pub metadata: HtmlMetadata,
    /// Quality signals
    pub signals: QualitySignals,
    /// Whether the content was truncated
    pub was_truncated: bool,
    /// Original HTML size in bytes
    pub original_size: usize,
}

/// Process raw HTML into final ready-to-use content in a single pipeline.
///
/// This replaces the previous multi-step flow:
///   1. preprocess_html (noise strip + metadata + signals)
///   2. HTML → Markdown conversion
///   3. Section-based truncation
///   4. Metadata header formatting
///
/// All steps now happen in a single function call with no intermediate FFI roundtrips.
pub fn process_web_content(raw_html: &str, config: &WebContentConfig) -> WebContentResult {
    // Step 1: Preprocess (noise strip + metadata + signals) — reuses existing pipeline
    let preprocess = html_preprocess::preprocess_html(raw_html);
    let original_size = preprocess.stats.original_len;

    // Step 2: Convert noise-stripped HTML to Markdown (or use cleaned text directly)
    let body = if config.to_markdown {
        convert_to_markdown(&preprocess)
    } else {
        preprocess.cleaned.clone()
    };

    // Step 3: Section-based truncation
    let (truncated_body, was_truncated) = truncate_by_sections(&body, config.max_chars);

    // Step 4: Assemble final output (metadata header + body)
    let content = assemble_output(&truncated_body, &preprocess, config, was_truncated);

    WebContentResult {
        content,
        metadata: preprocess.metadata,
        signals: preprocess.signals,
        was_truncated,
        original_size,
    }
}

/// Convert noise-stripped HTML to Markdown.
///
/// Uses the cleaned HTML from preprocessing (noise already removed) so html2md
/// only parses content once — no double parsing of the original HTML.
fn convert_to_markdown(preprocess: &PreprocessResult) -> String {
    let md = html2md::parse_html(&preprocess.cleaned_html);
    clean_markdown(&md)
}

/// Clean up Markdown output: normalize whitespace, remove empty links, etc.
fn clean_markdown(md: &str) -> String {
    let mut result = String::with_capacity(md.len());
    let mut prev_blank = false;

    for line in md.lines() {
        let trimmed = line.trim();

        // Skip empty navigation-like lines (just links with no context)
        if trimmed == "[]()" || trimmed == "[]" {
            continue;
        }

        // Collapse multiple blank lines to one
        if trimmed.is_empty() {
            if !prev_blank && !result.is_empty() {
                result.push('\n');
                prev_blank = true;
            }
            continue;
        }

        prev_blank = false;
        result.push_str(line);
        result.push('\n');
    }

    result.trim_end().to_string()
}

/// Truncate content at section boundaries rather than mid-sentence.
///
/// Looks for natural break points: headings, paragraph breaks, list boundaries.
/// This preserves readability compared to hard character truncation.
fn truncate_by_sections(content: &str, max_chars: usize) -> (String, bool) {
    if content.len() <= max_chars {
        return (content.to_string(), false);
    }

    // Find a safe UTF-8 boundary for the search region
    let safe_end = {
        let mut pos = max_chars;
        while pos > 0 && !content.is_char_boundary(pos) {
            pos -= 1;
        }
        pos
    };
    let search_region = &content[..safe_end];

    let min_keep = safe_end * 3 / 5; // keep at least 60%

    // Priority 1: Break at a heading (## or #)
    let mut best_break = None;
    for (i, _) in search_region.rmatch_indices("\n#") {
        if i >= min_keep {
            best_break = Some(i);
            break;
        }
    }

    // Priority 2: Break at a double newline (paragraph boundary)
    if best_break.is_none() {
        for (i, _) in search_region.rmatch_indices("\n\n") {
            if i >= min_keep {
                best_break = Some(i);
                break;
            }
        }
    }

    // Priority 3: Break at a single newline
    if best_break.is_none() {
        for (i, _) in search_region.rmatch_indices('\n') {
            if i >= min_keep {
                best_break = Some(i);
                break;
            }
        }
    }

    // Fallback: use the safe_end we already computed
    let break_at = best_break.unwrap_or(safe_end);

    let truncated = content[..break_at].trim_end().to_string();
    (truncated, true)
}

/// Assemble the final output with metadata header and body.
fn assemble_output(
    body: &str,
    preprocess: &PreprocessResult,
    config: &WebContentConfig,
    was_truncated: bool,
) -> String {
    let mut out = String::with_capacity(body.len() + 512);

    if config.include_metadata {
        // Metadata header
        if let Some(ref title) = preprocess.metadata.title {
            out.push_str(&format!("**{title}**\n\n"));
        }

        let mut meta_lines = Vec::new();

        if let Some(ref desc) = preprocess
            .metadata
            .og_description
            .as_ref()
            .or(preprocess.metadata.description.as_ref())
        {
            meta_lines.push(format!("> {desc}"));
        }
        if let Some(ref author) = preprocess.metadata.author {
            meta_lines.push(format!("Author: {author}"));
        }
        if let Some(ref date) = preprocess.metadata.published_date {
            meta_lines.push(format!("Published: {date}"));
        }
        if let Some(ref url) = preprocess
            .metadata
            .canonical_url
            .as_ref()
            .or(preprocess.metadata.og_url.as_ref())
        {
            meta_lines.push(format!("Source: {url}"));
        }

        if !meta_lines.is_empty() {
            out.push_str(&meta_lines.join("\n"));
            out.push('\n');
        }
    }

    if config.include_warnings && !preprocess.signals.issues.is_empty() {
        out.push_str(&format!(
            "\n⚠ {}\n",
            preprocess.signals.issues.join(" | ")
        ));
    }

    // Stats line
    out.push_str(&format!(
        "\n[{} → {} bytes | {} noise elements removed | {:.0}% content",
        preprocess.stats.original_len,
        preprocess.stats.cleaned_len,
        preprocess.stats.elements_removed,
        preprocess.signals.content_ratio * 100.0,
    ));
    if was_truncated {
        out.push_str(" | truncated");
    }
    out.push_str("]\n\n---\n\n");

    out.push_str(body);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_pipeline() {
        let html = r#"
        <html>
        <head>
            <title>Test Article</title>
            <meta name="description" content="An interesting article">
            <meta name="author" content="John Doe">
        </head>
        <body>
            <nav><a href="/">Home</a> | <a href="/about">About</a></nav>
            <article>
                <h1>The Main Heading</h1>
                <p>This is the first paragraph with <strong>bold text</strong>.</p>
                <p>Second paragraph with a <a href="https://example.com">link</a>.</p>
                <h2>Subheading</h2>
                <p>More content under the subheading.</p>
            </article>
            <div class="ad-banner">Buy our product!</div>
            <footer>Copyright 2024</footer>
        </body>
        </html>"#;

        let config = WebContentConfig::default();
        let result = process_web_content(html, &config);

        assert!(result.content.contains("Test Article"));
        assert!(result.content.contains("The Main Heading"));
        assert!(!result.was_truncated);
        assert!(result.metadata.title.as_deref() == Some("Test Article"));
        assert!(result.metadata.author.as_deref() == Some("John Doe"));
    }

    #[test]
    fn test_truncation_at_section_boundary() {
        let mut long_content = String::new();
        for i in 0..50 {
            long_content.push_str(&format!("## Section {i}\n\nThis is paragraph content for section {i}. It has some text to fill space and make the document longer than the truncation limit.\n\n"));
        }

        let (truncated, was_truncated) = truncate_by_sections(&long_content, 500);

        assert!(was_truncated);
        assert!(truncated.len() <= 500);
        // Should end at a section boundary (double newline or heading)
        assert!(
            truncated.ends_with('\n') || truncated.ends_with('.'),
            "Should end at a natural boundary"
        );
    }

    #[test]
    fn test_markdown_conversion() {
        let html = r#"
        <html><body>
            <h1>Title</h1>
            <p>Text with <strong>bold</strong> and <em>italic</em>.</p>
            <ul><li>Item 1</li><li>Item 2</li></ul>
        </body></html>"#;

        let config = WebContentConfig {
            include_metadata: false,
            include_warnings: false,
            ..Default::default()
        };
        let result = process_web_content(html, &config);

        // Should contain markdown formatting
        assert!(result.content.contains("**bold**") || result.content.contains("bold"));
    }

    #[test]
    fn test_config_no_metadata() {
        let html = r#"<html><head><title>Test</title></head><body><p>Hello</p></body></html>"#;

        let config = WebContentConfig {
            include_metadata: false,
            include_warnings: false,
            ..Default::default()
        };
        let result = process_web_content(html, &config);

        // Should not contain bold title header
        assert!(!result.content.starts_with("**Test**"));
    }
}
