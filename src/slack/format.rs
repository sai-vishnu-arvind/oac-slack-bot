/// Convert standard Markdown to Slack mrkdwn format.
///
/// Transformations:
/// - Headings (`# text`) → bold text with spacing
/// - Bold (`**text**`, `__text__`) → `*text*`
/// - Italic (`*text*`) → `_text_`
/// - Strikethrough (`~~text~~`) → `~text~`
/// - Links (`[text](url)`) → `<url|text>`
/// - Images (`![alt](url)`) → `<url>`
/// - Unordered lists (`- `, `* `, `+ `) → `•`
/// - Horizontal rules (`---`, `***`, `___`) → divider line
///
/// Preserves code blocks (```) and inline code (`` ` ``).
pub fn md_to_slack(md: &str) -> String {
    if md.is_empty() {
        return String::new();
    }

    let mut result = String::with_capacity(md.len());
    let mut in_code_block = false;
    let mut table_lines: Vec<String> = Vec::new();

    for line in md.lines() {
        let trimmed = line.trim();

        // Toggle code block state
        if trimmed.starts_with("```") {
            // Flush any pending table before code block
            if !table_lines.is_empty() {
                result.push_str(&format_table(&table_lines));
                table_lines.clear();
            }
            in_code_block = !in_code_block;
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Inside code block — pass through unchanged
        if in_code_block {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Detect table rows: lines that start and end with `|`
        if is_table_row(trimmed) {
            table_lines.push(trimmed.to_string());
            continue;
        }

        // If we were accumulating a table and this line isn't a table row, flush it
        if !table_lines.is_empty() {
            result.push_str(&format_table(&table_lines));
            table_lines.clear();
        }

        // Blank line
        if trimmed.is_empty() {
            result.push('\n');
            continue;
        }

        // Horizontal rule
        if is_hr(trimmed) {
            result.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
            continue;
        }

        // Heading → bold with a blank line before for visual separation
        if let Some(heading_text) = strip_heading(trimmed) {
            let converted = convert_inline(heading_text);
            result.push_str(&format!("\n*{}*\n", converted));
            continue;
        }

        // Regular line: list markers + inline formatting
        let converted = convert_list_marker(line);
        let converted = convert_inline(&converted);

        result.push_str(&converted);
        result.push('\n');
    }

    // Flush any trailing table
    if !table_lines.is_empty() {
        result.push_str(&format_table(&table_lines));
    }

    // Remove trailing newline if the original didn't have one
    if result.ends_with('\n') && !md.ends_with('\n') {
        result.pop();
    }

    result
}

// ── Table conversion ────────────────────────────────────────────────────────

/// Check if a line looks like a markdown table row.
fn is_table_row(trimmed: &str) -> bool {
    trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() >= 3
}

/// Check if a row is a separator (e.g. `|---|---|---|`).
fn is_separator_row(row: &str) -> bool {
    row.split('|')
        .filter(|cell| !cell.is_empty())
        .all(|cell| cell.trim().chars().all(|c| c == '-' || c == ':' || c == ' '))
}

/// Parse a table row into trimmed cell strings, stripping markdown formatting
/// since table cells render inside a code block where markers are noise.
fn parse_table_cells(row: &str) -> Vec<String> {
    let stripped = row.strip_prefix('|').unwrap_or(row);
    let stripped = stripped.strip_suffix('|').unwrap_or(stripped);
    stripped
        .split('|')
        .map(|c| strip_cell_formatting(c.trim()))
        .collect()
}

/// Remove markdown formatting from a table cell value.
/// Strips: `backticks`, **bold**, *italic*, [link](url) → link text.
fn strip_cell_formatting(cell: &str) -> String {
    let mut s = cell.to_string();

    // Strip inline code backticks: `text` → text
    while let (Some(start), Some(end)) = (s.find('`'), s.rfind('`')) {
        if start < end {
            let inner = s[start + 1..end].to_string();
            s = format!("{}{}{}", &s[..start], inner, &s[end + 1..]);
        } else {
            break;
        }
    }

    // Strip bold: **text** → text
    while s.contains("**") {
        if let Some(start) = s.find("**") {
            let after = &s[start + 2..];
            if let Some(end) = after.find("**") {
                let inner = after[..end].to_string();
                s = format!("{}{}{}", &s[..start], inner, &after[end + 2..]);
            } else {
                break;
            }
        } else {
            break;
        }
    }

    // Strip single italic: *text* → text (after bold is gone)
    while s.contains('*') {
        if let Some(start) = s.find('*') {
            let after = &s[start + 1..];
            if let Some(end) = after.find('*') {
                let inner = after[..end].to_string();
                s = format!("{}{}{}", &s[..start], inner, &after[end + 1..]);
            } else {
                break;
            }
        } else {
            break;
        }
    }

    // Strip links: [text](url) → text
    while s.contains("](") {
        if let Some(bracket_start) = s.find('[') {
            if let Some(bracket_end) = s[bracket_start..].find("](") {
                let text = &s[bracket_start + 1..bracket_start + bracket_end];
                // Find the closing )
                let url_start = bracket_start + bracket_end + 2;
                if let Some(paren_end) = s[url_start..].find(')') {
                    s = format!(
                        "{}{}{}",
                        &s[..bracket_start],
                        text,
                        &s[url_start + paren_end + 1..]
                    );
                } else {
                    break;
                }
            } else {
                break;
            }
        } else {
            break;
        }
    }

    s
}

/// Convert markdown table lines into a Slack-friendly code block with aligned columns.
fn format_table(lines: &[String]) -> String {
    // Parse all data rows (skip separator rows).
    let data_rows: Vec<Vec<String>> = lines
        .iter()
        .filter(|line| !is_separator_row(line))
        .map(|line| parse_table_cells(line))
        .collect();

    if data_rows.is_empty() {
        return String::new();
    }

    // Calculate the max width for each column.
    let col_count = data_rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut col_widths = vec![0usize; col_count];
    for row in &data_rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                col_widths[i] = col_widths[i].max(cell.len());
            }
        }
    }

    // Build the code block.
    let mut out = String::from("```\n");

    for (row_idx, row) in data_rows.iter().enumerate() {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                line.push_str("  |  ");
            }
            let width = col_widths.get(i).copied().unwrap_or(0);
            line.push_str(&format!("{:<width$}", cell, width = width));
        }
        out.push_str(line.trim_end());
        out.push('\n');

        // Add a clean separator line after the header row.
        if row_idx == 0 && data_rows.len() > 1 {
            let mut sep = String::new();
            for (i, &w) in col_widths.iter().enumerate() {
                if i > 0 {
                    sep.push_str("--+--");
                }
                for _ in 0..w {
                    sep.push('-');
                }
            }
            out.push_str(&sep);
            out.push('\n');
        }
    }

    out.push_str("```\n");
    out
}

/// Check if a trimmed line is a horizontal rule (---, ***, ___).
fn is_hr(trimmed: &str) -> bool {
    if trimmed.len() < 3 {
        return false;
    }
    let marker = match trimmed.chars().find(|c| *c != ' ') {
        Some(c @ ('-' | '*' | '_')) => c,
        _ => return false,
    };
    let marker_count = trimmed.chars().filter(|c| *c == marker).count();
    let all_valid = trimmed.chars().all(|c| c == marker || c == ' ');
    marker_count >= 3 && all_valid
}

/// Strip heading markers: `## Hello` → `Some("Hello")`.
fn strip_heading(trimmed: &str) -> Option<&str> {
    if !trimmed.starts_with('#') {
        return None;
    }
    let without = trimmed.trim_start_matches('#');
    // Standard Markdown requires a space after the hashes
    if without.starts_with(' ') {
        Some(without.trim_start())
    } else {
        None
    }
}

/// Convert Markdown list markers to bullet points.
fn convert_list_marker(line: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let trimmed = line.trim_start();

    // Unordered: `- `, `* `, `+ `
    for prefix in &["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let spaces: String = "  ".repeat(indent / 2);
            return format!("{}•  {}", spaces, rest);
        }
    }

    line.to_string()
}

/// Convert inline Markdown formatting to Slack mrkdwn.
/// Preserves content inside backtick code spans.
fn convert_inline(text: &str) -> String {
    // 1. Protect inline code spans
    let (protected, code_spans) = protect_code_spans(text);

    // 2. Convert images before links (images start with `!`)
    let s = convert_images(&protected);

    // 3. Convert links
    let s = convert_links(&s);

    // 4. Convert bold / italic / strikethrough
    let s = convert_emphasis(&s);

    // 5. Restore code spans
    restore_code_spans(&s, &code_spans)
}

// ── Code span protection ────────────────────────────────────────────────────

/// Replace inline `` `code` `` spans with placeholders so the inner text
/// isn't mangled by the emphasis converter. Returns the modified string
/// plus a vec of the original spans.
fn protect_code_spans(text: &str) -> (String, Vec<String>) {
    let mut result = String::with_capacity(text.len());
    let mut spans: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '`' {
            let mut j = i + 1;
            let mut code = String::new();
            let mut closed = false;
            while j < chars.len() {
                if chars[j] == '`' {
                    closed = true;
                    j += 1;
                    break;
                }
                code.push(chars[j]);
                j += 1;
            }
            if closed {
                let idx = spans.len();
                spans.push(format!("`{}`", code));
                // Use a placeholder that won't appear in normal text
                result.push_str(&format!("\x00C{}\x00", idx));
                i = j;
            } else {
                // Unmatched backtick — keep as-is
                result.push('`');
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    (result, spans)
}

fn restore_code_spans(text: &str, spans: &[String]) -> String {
    let mut result = text.to_string();
    for (i, span) in spans.iter().enumerate() {
        let placeholder = format!("\x00C{}\x00", i);
        result = result.replace(&placeholder, span);
    }
    result
}

// ── Images and links ────────────────────────────────────────────────────────

/// `![alt](url)` → `<url>`
fn convert_images(text: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '!' && chars[i + 1] == '[' {
            if let Some((_alt, url, end)) = parse_md_link(&chars, i + 1) {
                result.push_str(&format!("<{}>", url));
                i = end;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

/// `[text](url)` → `<url|text>`
fn convert_links(text: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '[' {
            if let Some((link_text, url, end)) = parse_md_link(&chars, i) {
                if link_text.is_empty() {
                    result.push_str(&format!("<{}>", url));
                } else {
                    result.push_str(&format!("<{}|{}>", url, link_text));
                }
                i = end;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

/// Parse a Markdown link: `[text](url)` starting at the `[`.
/// Returns `(text, url, end_index)` or `None`.
fn parse_md_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    if start >= chars.len() || chars[start] != '[' {
        return None;
    }

    // Find closing `]`
    let mut i = start + 1;
    let mut link_text = String::new();
    let mut depth = 1;
    while i < chars.len() && depth > 0 {
        match chars[i] {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        link_text.push(chars[i]);
        i += 1;
    }
    if depth != 0 {
        return None;
    }

    // Expect `(` immediately after `]`
    i += 1;
    if i >= chars.len() || chars[i] != '(' {
        return None;
    }

    // Find closing `)`
    i += 1;
    let mut url = String::new();
    let mut paren_depth = 1;
    while i < chars.len() && paren_depth > 0 {
        match chars[i] {
            '(' => paren_depth += 1,
            ')' => {
                paren_depth -= 1;
                if paren_depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        url.push(chars[i]);
        i += 1;
    }
    if paren_depth != 0 {
        return None;
    }

    Some((link_text, url.trim().to_string(), i + 1))
}

// ── Emphasis (bold / italic / strikethrough) ────────────────────────────────

/// Convert `**bold**` → `*bold*`, `*italic*` → `_italic_`,
/// `~~strike~~` → `~strike~`.
///
/// Uses a placeholder technique so that the `*` output from bold conversion
/// doesn't get picked up by the italic pass.
fn convert_emphasis(text: &str) -> String {
    let mut s = text.to_string();

    // Strikethrough: ~~text~~ → ~text~
    s = replace_paired(&s, "~~", "~~", "~", "~");

    // Bold+italic: ***text*** → placeholder pair
    s = replace_paired(&s, "***", "***", "\x02\x03", "\x03\x02");

    // Bold: **text** → placeholder pair
    s = replace_paired(&s, "**", "**", "\x04", "\x05");

    // Bold: __text__ → placeholder pair
    s = replace_paired(&s, "__", "__", "\x04", "\x05");

    // Italic: single *text* → _text_
    // (safe because all ** pairs are now placeholders)
    s = convert_single_asterisk_italic(&s);

    // Restore placeholders
    s = s.replace("\x02\x03", "*_"); // bold+italic open
    s = s.replace("\x03\x02", "_*"); // bold+italic close
    s = s.replace('\x04', "*");      // bold open
    s = s.replace('\x05', "*");      // bold close

    s
}

/// Find non-overlapping `open…close` pairs and replace markers.
fn replace_paired(text: &str, open: &str, close: &str, new_open: &str, new_close: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;

    while let Some(start) = remaining.find(open) {
        result.push_str(&remaining[..start]);
        let after = &remaining[start + open.len()..];

        if let Some(end) = after.find(close) {
            let inner = &after[..end];
            // Only convert if there's non-whitespace content
            if !inner.is_empty() && !inner.chars().all(char::is_whitespace) {
                result.push_str(new_open);
                result.push_str(inner);
                result.push_str(new_close);
            } else {
                result.push_str(open);
                result.push_str(inner);
                result.push_str(close);
            }
            remaining = &after[end + close.len()..];
        } else {
            // No closing marker — keep the rest as-is
            result.push_str(&remaining[start..]);
            remaining = "";
            break;
        }
    }

    result.push_str(remaining);
    result
}

/// Convert remaining single `*text*` to `_text_` (italic).
fn convert_single_asterisk_italic(text: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '*' {
            // Look for closing `*` on the same line
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '*' && chars[j] != '\n' {
                j += 1;
            }
            if j < chars.len() && chars[j] == '*' && j > i + 1 {
                result.push('_');
                for k in (i + 1)..j {
                    result.push(chars[k]);
                }
                result.push('_');
                i = j + 1;
            } else {
                result.push(chars[i]);
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Headings ────────────────────────────────────────────────────────────

    #[test]
    fn heading_h1() {
        assert_eq!(md_to_slack("# Hello"), "\n*Hello*");
    }

    #[test]
    fn heading_h3() {
        assert_eq!(md_to_slack("### Sub-heading"), "\n*Sub-heading*");
    }

    #[test]
    fn heading_not_a_heading() {
        // No space after `#` → not a heading
        assert_eq!(md_to_slack("#hashtag"), "#hashtag");
    }

    // ── Bold / italic / strikethrough ───────────────────────────────────────

    #[test]
    fn bold_double_asterisk() {
        assert_eq!(md_to_slack("**bold**"), "*bold*");
    }

    #[test]
    fn bold_double_underscore() {
        assert_eq!(md_to_slack("__bold__"), "*bold*");
    }

    #[test]
    fn italic_single_asterisk() {
        assert_eq!(md_to_slack("*italic*"), "_italic_");
    }

    #[test]
    fn bold_and_italic_in_same_line() {
        assert_eq!(
            md_to_slack("**bold** and *italic*"),
            "*bold* and _italic_"
        );
    }

    #[test]
    fn bold_italic_triple_asterisk() {
        assert_eq!(md_to_slack("***both***"), "*_both_*");
    }

    #[test]
    fn strikethrough() {
        assert_eq!(md_to_slack("~~deleted~~"), "~deleted~");
    }

    // ── Links ───────────────────────────────────────────────────────────────

    #[test]
    fn markdown_link() {
        assert_eq!(
            md_to_slack("[click here](https://example.com)"),
            "<https://example.com|click here>"
        );
    }

    #[test]
    fn image_link() {
        assert_eq!(
            md_to_slack("![logo](https://img.example.com/logo.png)"),
            "<https://img.example.com/logo.png>"
        );
    }

    // ── Lists ───────────────────────────────────────────────────────────────

    #[test]
    fn unordered_list() {
        let input = "- first\n- second\n- third";
        let expected = "•  first\n•  second\n•  third";
        assert_eq!(md_to_slack(input), expected);
    }

    #[test]
    fn ordered_list_preserved() {
        let input = "1. first\n2. second";
        assert_eq!(md_to_slack(input), input);
    }

    // ── Horizontal rules ────────────────────────────────────────────────────

    #[test]
    fn horizontal_rule_dashes() {
        assert_eq!(md_to_slack("---"), "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    }

    #[test]
    fn horizontal_rule_asterisks() {
        assert_eq!(md_to_slack("***"), "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    }

    // ── Code blocks ─────────────────────────────────────────────────────────

    #[test]
    fn code_block_preserved() {
        let input = "```rust\nlet x = **not bold**;\n```";
        assert_eq!(md_to_slack(input), input);
    }

    #[test]
    fn inline_code_preserved() {
        assert_eq!(
            md_to_slack("use `**raw**` in code"),
            "use `**raw**` in code"
        );
    }

    // ── Block quotes ────────────────────────────────────────────────────────

    #[test]
    fn blockquote_passthrough() {
        // Slack already supports `>` for quotes
        assert_eq!(md_to_slack("> a quote"), "> a quote");
    }

    // ── Mixed content ───────────────────────────────────────────────────────

    #[test]
    fn full_message() {
        let input = "\
# Summary

Here is **bold** and *italic* text.

- Item one
- Item two with [a link](https://example.com)

---

> Quote block

```
code block
```";

        let output = md_to_slack(input);

        // Spot-check key transformations
        assert!(output.contains("*Summary*"), "heading");
        assert!(output.contains("*bold*"), "bold");
        assert!(output.contains("_italic_"), "italic");
        assert!(output.contains("•  Item one"), "list");
        assert!(output.contains("<https://example.com|a link>"), "link");
        assert!(output.contains("━━━"), "hr");
        assert!(output.contains("> Quote block"), "quote");
        assert!(output.contains("code block"), "code");
    }

    // ── Edge cases ──────────────────────────────────────────────────────────

    #[test]
    fn empty_input() {
        assert_eq!(md_to_slack(""), "");
    }

    #[test]
    fn unmatched_bold_marker() {
        // No closing ** → keep as-is
        assert_eq!(md_to_slack("**unfinished"), "**unfinished");
    }

    #[test]
    fn unmatched_backtick() {
        assert_eq!(md_to_slack("`unfinished code"), "`unfinished code");
    }

    #[test]
    fn multiple_bold_phrases() {
        assert_eq!(
            md_to_slack("**a** then **b**"),
            "*a* then *b*"
        );
    }

    #[test]
    fn link_with_bold_text() {
        assert_eq!(
            md_to_slack("[**bold link**](https://x.com)"),
            "<https://x.com|*bold link*>"
        );
    }

    #[test]
    fn heading_with_inline_formatting() {
        assert_eq!(
            md_to_slack("## A **bold** heading"),
            "\n*A *bold* heading*"
        );
    }

    #[test]
    fn nested_list_indentation() {
        let input = "- outer\n  - inner";
        let output = md_to_slack(input);
        assert!(output.contains("•  outer"));
        assert!(output.contains("•  inner"));
    }

    // ── Tables ──────────────────────────────────────────────────────────────

    #[test]
    fn simple_table() {
        let input = "\
| Name | Value |
|------|-------|
| foo  | 123   |
| bar  | 456   |";
        let output = md_to_slack(input);
        assert!(output.contains("```"), "table should be in code block");
        assert!(output.contains("foo"), "data preserved");
        assert!(output.contains("bar"), "data preserved");
        assert!(output.contains("|"), "columns separated");
        assert!(!output.contains("|---"), "separator row removed");
    }

    #[test]
    fn table_with_empty_cells() {
        let input = "\
| | Failure | Success |
|---|---|---|
| ID | abc | xyz |";
        let output = md_to_slack(input);
        assert!(output.contains("```"));
        assert!(output.contains("Failure"));
        assert!(output.contains("abc"));
    }

    #[test]
    fn table_surrounded_by_text() {
        let input = "\
Some text before.

| A | B |
|---|---|
| 1 | 2 |

Some text after.";
        let output = md_to_slack(input);
        assert!(output.contains("Some text before."));
        assert!(output.contains("```"));
        assert!(output.contains("Some text after."));
    }

    #[test]
    fn not_a_table_just_pipes() {
        // Single pipe line shouldn't be treated as table
        let input = "use | for OR";
        let output = md_to_slack(input);
        assert!(!output.contains("```"));
    }

    // ── Edge cases: lists with inline formatting ────────────────────────

    #[test]
    fn bold_inside_list_item() {
        // Very common Claude output: `- **Key**: value`
        let output = md_to_slack("- **Key**: value");
        assert!(output.contains("•  *Key*: value"), "got: {}", output);
    }

    #[test]
    fn asterisk_list_marker_with_bold() {
        // `* ` is a list marker, `**item**` is bold — should not conflict
        let output = md_to_slack("* **item**");
        assert!(output.contains("•"), "should be bullet, got: {}", output);
        assert!(output.contains("*item*"), "bold converted, got: {}", output);
    }

    #[test]
    fn link_inside_list_item() {
        let output = md_to_slack("- See [docs](https://x.com)");
        assert_eq!(output, "•  See <https://x.com|docs>");
    }

    // ── Edge cases: code blocks ─────────────────────────────────────────

    #[test]
    fn table_inside_code_block_not_converted() {
        let input = "```\n| A | B |\n|---|---|\n| 1 | 2 |\n```";
        let output = md_to_slack(input);
        // Table syntax should be preserved raw inside code block
        assert!(output.contains("| A | B |"), "got: {}", output);
        // Should NOT have the --+-- separator that format_table produces
        assert!(!output.contains("--+--"), "should not reformat, got: {}", output);
    }

    #[test]
    fn unclosed_code_block() {
        // Everything after ``` should stay raw (no bold conversion)
        let input = "```\ncode\n**not bold**";
        let output = md_to_slack(input);
        assert!(output.contains("**not bold**"), "should be raw, got: {}", output);
    }

    #[test]
    fn multiple_code_blocks() {
        let input = "```\na\n```\n**bold**\n```\nb\n```";
        let output = md_to_slack(input);
        assert!(output.contains("*bold*"), "bold between code blocks, got: {}", output);
    }

    // ── Edge cases: tables ──────────────────────────────────────────────

    #[test]
    fn bold_markers_inside_table_stripped() {
        // Markdown formatting stripped from table cells since they're in code block
        let input = "| **Name** | Value |\n|---|---|\n| foo | bar |";
        let output = md_to_slack(input);
        assert!(output.contains("```"), "should be code block");
        assert!(output.contains("Name"), "cell text present");
        assert!(!output.contains("**Name**"), "bold markers stripped, got: {}", output);
    }

    #[test]
    fn single_row_table() {
        // Just a header row, no separator or data
        let input = "| Just | One | Row |";
        let output = md_to_slack(input);
        assert!(output.contains("```"), "should still be code block");
        assert!(output.contains("Just"), "got: {}", output);
    }

    #[test]
    fn consecutive_tables_separated_by_blank() {
        let input = "| A |\n|---|\n| 1 |\n\n| B |\n|---|\n| 2 |";
        let output = md_to_slack(input);
        // Should produce two separate code blocks
        let code_block_count = output.matches("```").count();
        assert!(code_block_count >= 4, "two tables = 4+ fences, got {}: {}", code_block_count, output);
    }

    #[test]
    fn heading_then_table() {
        let input = "# Title\n| A | B |\n|---|---|\n| 1 | 2 |";
        let output = md_to_slack(input);
        assert!(output.contains("*Title*"), "heading converted");
        assert!(output.contains("```"), "table in code block");
    }

    #[test]
    fn solo_separator_row() {
        // A separator row alone (|---|---|) — technically a table row
        let input = "|---|---|";
        let output = md_to_slack(input);
        // Should produce empty since separator-only table has no data
        assert!(!output.contains("│"), "no data rows, got: {}", output);
    }

    // ── Edge cases: blockquotes ─────────────────────────────────────────

    #[test]
    fn blockquote_with_bold() {
        let output = md_to_slack("> **important** note");
        assert!(output.contains("> *important* note"), "got: {}", output);
    }

    // ── Edge cases: emphasis ────────────────────────────────────────────

    #[test]
    fn empty_bold_markers() {
        // **** should not crash or produce weird output
        let output = md_to_slack("****");
        assert!(!output.is_empty());
    }

    #[test]
    fn inline_code_next_to_bold() {
        let output = md_to_slack("`code` and **bold**");
        assert!(output.contains("`code`"), "code preserved");
        assert!(output.contains("*bold*"), "bold converted, got: {}", output);
    }

    #[test]
    fn link_with_query_params() {
        let output = md_to_slack("[click](https://x.com/a?b=1&c=2)");
        assert!(output.contains("<https://x.com/a?b=1&c=2|click>"), "got: {}", output);
    }

    // ── Edge cases: whitespace ──────────────────────────────────────────

    #[test]
    fn multi_paragraph() {
        let output = md_to_slack("para one\n\npara two");
        assert!(output.contains("para one\n\npara two"), "got: {}", output);
    }

    #[test]
    fn whitespace_only_line() {
        let output = md_to_slack("text\n   \nmore");
        assert!(output.contains("text") && output.contains("more"), "got: {}", output);
    }

    // ── Edge cases: deeply indented list ────────────────────────────────

    #[test]
    fn deeply_indented_list() {
        let output = md_to_slack("    - deep");
        assert!(output.contains("•  deep"), "got: {}", output);
    }

    // ── Edge cases: numbered list with formatting ───────────────────────

    #[test]
    fn numbered_list_with_bold() {
        let output = md_to_slack("1. **First** item\n2. Second");
        assert!(output.contains("*First*"), "bold in numbered list, got: {}", output);
        assert!(output.contains("1."), "number preserved");
    }

    // ── Edge cases: mixed slash content ─────────────────────────────────

    #[test]
    fn realistic_claude_output() {
        let input = "\
## Root Cause Analysis

The issue is caused by **wrong identifier** passed to the API.

| Field | Failure | Success |
|-------|---------|---------|
| ID    | abc     | xyz     |

### Recommendations

1. **Fix the identifier** — use `mihpayid` instead of `bank_ref_num`
2. Add validation to catch this early
- Check the [API docs](https://docs.example.com)

> Note: This is a known issue.

```json
{\"var1\": \"correct_value\"}
```";
        let output = md_to_slack(input);

        // Headings
        assert!(output.contains("*Root Cause Analysis*"), "h2");
        assert!(output.contains("*Recommendations*"), "h3");
        // Bold
        assert!(output.contains("*wrong identifier*"), "bold");
        assert!(output.contains("*Fix the identifier*"), "bold in list");
        // Table → code block
        assert!(output.contains("```\n"), "table as code block");
        assert!(output.contains("Failure"), "table data");
        assert!(!output.contains("|---"), "no raw separator");
        // Inline code
        assert!(output.contains("`mihpayid`"), "inline code");
        // Link
        assert!(output.contains("<https://docs.example.com|API docs>"), "link");
        // Quote
        assert!(output.contains("> Note:"), "blockquote");
        // Code block preserved
        assert!(output.contains("\"var1\""), "json code block");
    }
}
