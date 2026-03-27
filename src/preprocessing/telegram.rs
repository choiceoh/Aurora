//! Telegram-compatible table formatting.
//!
//! Telegram doesn't support markdown tables (`| col | col |`). This module
//! detects markdown tables in text and converts them to monospace-aligned
//! `<pre>` blocks that render correctly in Telegram's HTML parse mode.

use unicode_width::UnicodeWidthStr;

/// Convert all markdown tables in `text` to Telegram-compatible `<pre>` blocks.
pub fn format_tables_for_telegram(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut result = String::with_capacity(text.len());
    let mut i = 0;

    while i < lines.len() {
        // Try to consume a markdown table starting at line i
        if let Some((table_str, end)) = try_parse_table(&lines, i) {
            result.push_str(&table_str);
            result.push('\n');
            i = end;
        } else {
            result.push_str(lines[i]);
            result.push('\n');
            i += 1;
        }
    }

    // Remove trailing newline added by the loop
    if result.ends_with('\n') && !text.ends_with('\n') {
        result.pop();
    }

    result
}

/// Try to parse a markdown table starting at `start`. Returns the formatted
/// table string and the index past the last table line, or `None` if
/// the lines at `start` don't form a valid table.
fn try_parse_table(lines: &[&str], start: usize) -> Option<(String, usize)> {
    // A markdown table needs at least 2 lines: header + separator
    if start + 1 >= lines.len() {
        return None;
    }

    let header_line = lines[start].trim();
    if !is_table_row(header_line) {
        return None;
    }

    // The second line must be a separator (e.g. |---|---|)
    let sep_line = lines[start + 1].trim();
    if !is_separator_row(sep_line) {
        return None;
    }

    // Parse header cells
    let header_cells = parse_row(header_line);
    let col_count = header_cells.len();
    if col_count == 0 {
        return None;
    }

    // Collect data rows
    let mut data_rows: Vec<Vec<String>> = Vec::new();
    let mut end = start + 2;
    while end < lines.len() {
        let row_line = lines[end].trim();
        if !is_table_row(row_line) {
            break;
        }
        let cells = parse_row(row_line);
        data_rows.push(cells);
        end += 1;
    }

    // Calculate column widths (using display width for CJK/emoji)
    let mut col_widths: Vec<usize> = header_cells
        .iter()
        .map(|c| display_width(c))
        .collect();

    for row in &data_rows {
        for (j, cell) in row.iter().enumerate() {
            if j < col_widths.len() {
                col_widths[j] = col_widths[j].max(display_width(cell));
            }
        }
    }

    // Build the formatted table
    let mut table = String::new();
    table.push_str("<pre>\n");

    // Header row
    table.push_str(&format_row(&header_cells, &col_widths));
    table.push('\n');

    // Separator row using ─
    let sep: Vec<String> = col_widths.iter().map(|&w| "─".repeat(w)).collect();
    table.push_str(&sep.join("─┼─"));
    table.push('\n');

    // Data rows
    for row in &data_rows {
        table.push_str(&format_row(row, &col_widths));
        table.push('\n');
    }

    table.push_str("</pre>");

    Some((table, end))
}

/// Check if a line looks like a markdown table row (contains `|`).
fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('|') && !trimmed.is_empty()
}

/// Check if a line is a markdown table separator row (e.g. `|---|---|`).
fn is_separator_row(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return false;
    }
    let inner = trimmed.trim_matches('|');
    inner
        .split('|')
        .all(|cell| {
            let c = cell.trim();
            !c.is_empty()
                && c.chars()
                    .all(|ch| ch == '-' || ch == ':' || ch == ' ')
        })
}

/// Parse a markdown table row into cells, trimming whitespace.
fn parse_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    // Remove leading/trailing pipe
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed);
    let inner = inner
        .strip_suffix('|')
        .unwrap_or(inner);

    inner
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

/// Format a row of cells with proper padding using display widths.
fn format_row(cells: &[String], col_widths: &[usize]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (j, width) in col_widths.iter().enumerate() {
        let cell = cells.get(j).map(|s| s.as_str()).unwrap_or("");
        let dw = display_width(cell);
        let padding = if *width >= dw { width - dw } else { 0 };
        parts.push(format!("{}{}", cell, " ".repeat(padding)));
    }
    parts.join(" │ ")
}

/// Calculate display width, accounting for wide CJK characters and emoji.
fn display_width(s: &str) -> usize {
    // UnicodeWidthStr handles CJK but not all emoji perfectly.
    // For emoji that are zero-width per unicode_width, count them as 2.
    let base = UnicodeWidthStr::width(s);
    if base > 0 {
        return base;
    }
    // Fallback: if the string is non-empty but width is 0 (e.g. all emoji),
    // estimate by counting chars * 2
    if !s.is_empty() {
        s.chars().count() * 2
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_table_conversion() {
        let input = "\
| Name | Score |
|---|---|
| Alice | 100 |
| Bob | 95 |";

        let result = format_tables_for_telegram(input);
        assert!(result.contains("<pre>"));
        assert!(result.contains("</pre>"));
        assert!(result.contains("Alice"));
        assert!(result.contains("─┼─"));
        assert!(!result.contains("|---|"));
    }

    #[test]
    fn test_cjk_table() {
        let input = "\
| 기능 | OpenClaw | Deneb |
|---|---|---|
| Polaris | ❌ | ✅ |
| Dreaming | ❌ | ✅ |";

        let result = format_tables_for_telegram(input);
        assert!(result.contains("<pre>"));
        assert!(result.contains("Polaris"));
        assert!(result.contains("Dreaming"));
    }

    #[test]
    fn test_text_around_table() {
        let input = "\
Some text before.

| A | B |
|---|---|
| 1 | 2 |

Some text after.";

        let result = format_tables_for_telegram(input);
        assert!(result.contains("Some text before."));
        assert!(result.contains("Some text after."));
        assert!(result.contains("<pre>"));
    }

    #[test]
    fn test_no_table() {
        let input = "Just normal text\nwith no table.";
        let result = format_tables_for_telegram(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_real_world_table() {
        let input = "\
솔직히 대단해.

| 기능 | OpenClaw | Deneb |
|---|---|---|
| Polaris (시스템 문서) | ❌ | ✅ 249문서 + Pilot 연동 |
| Dreaming (자가 메모리 정제) | ❌ | ✅ Gemini + 카테고리 가중치 |
| Autonomous (자율 사이클) | ❌ | ✅ goal 자동 주입까지 |

근데 제일 중요한 건 파이프라인이야";

        let result = format_tables_for_telegram(input);
        eprintln!("=== OUTPUT ===\n{result}\n=== END ===");

        assert!(result.contains("솔직히 대단해."));
        assert!(result.contains("근데 제일 중요한 건 파이프라인이야"));
        assert!(result.contains("<pre>"));
        assert!(result.contains("</pre>"));
        // No raw markdown table syntax
        assert!(!result.contains("|---|"));
    }

    #[test]
    fn test_separator_row_detection() {
        assert!(is_separator_row("|---|---|"));
        assert!(is_separator_row("| --- | --- |"));
        assert!(is_separator_row("|:---:|---:|"));
        assert!(!is_separator_row("| hello | world |"));
        assert!(!is_separator_row("not a separator"));
    }
}
