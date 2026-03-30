"""Convert standard Markdown to Slack mrkdwn format.

1:1 port of the Rust `src/slack/format.rs` implementation.
"""

from __future__ import annotations


def md_to_slack(md: str) -> str:
    """Convert Markdown to Slack mrkdwn."""
    if not md:
        return ""

    result: list[str] = []
    in_code_block = False
    table_lines: list[str] = []

    for line in md.split("\n"):
        trimmed = line.strip()

        # Toggle code block state
        if trimmed.startswith("```"):
            if table_lines:
                result.append(_format_table(table_lines))
                table_lines.clear()
            in_code_block = not in_code_block
            result.append(line)
            result.append("\n")
            continue

        # Inside code block — pass through
        if in_code_block:
            result.append(line)
            result.append("\n")
            continue

        # Table row detection
        if _is_table_row(trimmed):
            table_lines.append(trimmed)
            continue

        # Flush pending table
        if table_lines:
            result.append(_format_table(table_lines))
            table_lines.clear()

        # Blank line
        if not trimmed:
            result.append("\n")
            continue

        # Horizontal rule
        if _is_hr(trimmed):
            result.append("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n")
            continue

        # Heading -> bold
        heading_text = _strip_heading(trimmed)
        if heading_text is not None:
            converted = _convert_inline(heading_text)
            result.append(f"\n*{converted}*\n")
            continue

        # Regular line
        converted = _convert_list_marker(line)
        converted = _convert_inline(converted)
        result.append(converted)
        result.append("\n")

    # Flush trailing table
    if table_lines:
        result.append(_format_table(table_lines))

    text = "".join(result)

    # Remove trailing newline if original didn't have one
    if text.endswith("\n") and not md.endswith("\n"):
        text = text[:-1]

    return text


# ── Table conversion ──


def _is_table_row(trimmed: str) -> bool:
    return trimmed.startswith("|") and trimmed.endswith("|") and len(trimmed) >= 3


def _is_separator_row(row: str) -> bool:
    cells = [c for c in row.split("|") if c]
    return all(all(ch in "-: " for ch in cell) for cell in cells)


def _parse_table_cells(row: str) -> list[str]:
    stripped = row.strip("|")
    return [_strip_cell_formatting(c.strip()) for c in stripped.split("|")]


def _strip_cell_formatting(cell: str) -> str:
    s = cell

    # Strip inline code: `text` -> text
    while "`" in s:
        start = s.find("`")
        end = s.rfind("`")
        if start < end:
            s = s[:start] + s[start + 1 : end] + s[end + 1 :]
        else:
            break

    # Strip bold: **text** -> text
    while "**" in s:
        start = s.find("**")
        after = s[start + 2 :]
        end = after.find("**")
        if end >= 0:
            s = s[:start] + after[:end] + after[end + 2 :]
        else:
            break

    # Strip italic: *text* -> text
    while "*" in s:
        start = s.find("*")
        after = s[start + 1 :]
        end = after.find("*")
        if end >= 0:
            s = s[:start] + after[:end] + after[end + 1 :]
        else:
            break

    # Strip links: [text](url) -> text
    while "](" in s:
        bracket_start = s.find("[")
        if bracket_start < 0:
            break
        bracket_end = s.find("](", bracket_start)
        if bracket_end < 0:
            break
        text = s[bracket_start + 1 : bracket_end]
        url_start = bracket_end + 2
        paren_end = s.find(")", url_start)
        if paren_end < 0:
            break
        s = s[:bracket_start] + text + s[paren_end + 1 :]

    return s


def _format_table(lines: list[str]) -> str:
    data_rows = [_parse_table_cells(line) for line in lines if not _is_separator_row(line)]
    if not data_rows:
        return ""

    col_count = max(len(r) for r in data_rows)
    col_widths = [0] * col_count
    for row in data_rows:
        for i, cell in enumerate(row):
            if i < col_count:
                col_widths[i] = max(col_widths[i], len(cell))

    out = ["```\n"]
    for row_idx, row in enumerate(data_rows):
        parts: list[str] = []
        for i, cell in enumerate(row):
            if i > 0:
                parts.append("  |  ")
            width = col_widths[i] if i < len(col_widths) else 0
            parts.append(f"{cell:<{width}}")
        out.append("".join(parts).rstrip() + "\n")

        # Separator after header
        if row_idx == 0 and len(data_rows) > 1:
            sep_parts: list[str] = []
            for i, w in enumerate(col_widths):
                if i > 0:
                    sep_parts.append("--+--")
                sep_parts.append("-" * w)
            out.append("".join(sep_parts) + "\n")

    out.append("```\n")
    return "".join(out)


# ── Line helpers ──


def _is_hr(trimmed: str) -> bool:
    if len(trimmed) < 3:
        return False
    # Find first non-space char
    marker = None
    for ch in trimmed:
        if ch != " ":
            marker = ch
            break
    if marker not in ("-", "*", "_"):
        return False
    marker_count = trimmed.count(marker)
    all_valid = all(c == marker or c == " " for c in trimmed)
    return marker_count >= 3 and all_valid


def _strip_heading(trimmed: str) -> str | None:
    if not trimmed.startswith("#"):
        return None
    without = trimmed.lstrip("#")
    if without.startswith(" "):
        return without.lstrip()
    return None


def _convert_list_marker(line: str) -> str:
    indent = len(line) - len(line.lstrip())
    trimmed = line.lstrip()

    for prefix in ("- ", "* ", "+ "):
        if trimmed.startswith(prefix):
            rest = trimmed[len(prefix) :]
            spaces = "  " * (indent // 2)
            return f"{spaces}•  {rest}"

    return line


# ── Inline conversion ──


def _convert_inline(text: str) -> str:
    # 1. Protect code spans
    protected, code_spans = _protect_code_spans(text)
    # 2. Images
    s = _convert_images(protected)
    # 3. Links
    s = _convert_links(s)
    # 4. Emphasis
    s = _convert_emphasis(s)
    # 5. Restore code spans
    return _restore_code_spans(s, code_spans)


def _protect_code_spans(text: str) -> tuple[str, list[str]]:
    result: list[str] = []
    spans: list[str] = []
    chars = list(text)
    i = 0

    while i < len(chars):
        if chars[i] == "`":
            j = i + 1
            code: list[str] = []
            closed = False
            while j < len(chars):
                if chars[j] == "`":
                    closed = True
                    j += 1
                    break
                code.append(chars[j])
                j += 1
            if closed:
                idx = len(spans)
                spans.append(f"`{''.join(code)}`")
                result.append(f"\x00C{idx}\x00")
                i = j
            else:
                result.append("`")
                i += 1
        else:
            result.append(chars[i])
            i += 1

    return "".join(result), spans


def _restore_code_spans(text: str, spans: list[str]) -> str:
    result = text
    for i, span in enumerate(spans):
        result = result.replace(f"\x00C{i}\x00", span)
    return result


def _convert_images(text: str) -> str:
    result: list[str] = []
    chars = list(text)
    i = 0

    while i < len(chars):
        if i + 1 < len(chars) and chars[i] == "!" and chars[i + 1] == "[":
            parsed = _parse_md_link(chars, i + 1)
            if parsed:
                _, url, end = parsed
                result.append(f"<{url}>")
                i = end
                continue
        result.append(chars[i])
        i += 1

    return "".join(result)


def _convert_links(text: str) -> str:
    result: list[str] = []
    chars = list(text)
    i = 0

    while i < len(chars):
        if chars[i] == "[":
            parsed = _parse_md_link(chars, i)
            if parsed:
                link_text, url, end = parsed
                if not link_text:
                    result.append(f"<{url}>")
                else:
                    result.append(f"<{url}|{link_text}>")
                i = end
                continue
        result.append(chars[i])
        i += 1

    return "".join(result)


def _parse_md_link(chars: list[str], start: int) -> tuple[str, str, int] | None:
    if start >= len(chars) or chars[start] != "[":
        return None

    i = start + 1
    link_text: list[str] = []
    depth = 1
    while i < len(chars) and depth > 0:
        if chars[i] == "[":
            depth += 1
        elif chars[i] == "]":
            depth -= 1
            if depth == 0:
                break
        link_text.append(chars[i])
        i += 1

    if depth != 0:
        return None

    i += 1  # skip ']'
    if i >= len(chars) or chars[i] != "(":
        return None

    i += 1  # skip '('
    url: list[str] = []
    paren_depth = 1
    while i < len(chars) and paren_depth > 0:
        if chars[i] == "(":
            paren_depth += 1
        elif chars[i] == ")":
            paren_depth -= 1
            if paren_depth == 0:
                break
        url.append(chars[i])
        i += 1

    if paren_depth != 0:
        return None

    return "".join(link_text), "".join(url).strip(), i + 1


def _convert_emphasis(text: str) -> str:
    s = text

    # Strikethrough: ~~text~~ -> ~text~
    s = _replace_paired(s, "~~", "~~", "~", "~")

    # Bold+italic: ***text*** -> placeholder
    s = _replace_paired(s, "***", "***", "\x02\x03", "\x03\x02")

    # Bold: **text** -> placeholder
    s = _replace_paired(s, "**", "**", "\x04", "\x05")

    # Bold: __text__ -> placeholder
    s = _replace_paired(s, "__", "__", "\x04", "\x05")

    # Italic: *text* -> _text_
    s = _convert_single_asterisk_italic(s)

    # Restore placeholders
    s = s.replace("\x02\x03", "*_")  # bold+italic open
    s = s.replace("\x03\x02", "_*")  # bold+italic close
    s = s.replace("\x04", "*")  # bold open
    s = s.replace("\x05", "*")  # bold close

    return s


def _replace_paired(text: str, open_: str, close: str, new_open: str, new_close: str) -> str:
    result: list[str] = []
    remaining = text

    while True:
        start = remaining.find(open_)
        if start < 0:
            break

        result.append(remaining[:start])
        after = remaining[start + len(open_) :]

        end = after.find(close)
        if end < 0:
            result.append(remaining[start:])
            remaining = ""
            break

        inner = after[:end]
        if inner and not inner.isspace():
            result.append(new_open)
            result.append(inner)
            result.append(new_close)
        else:
            result.append(open_)
            result.append(inner)
            result.append(close)

        remaining = after[end + len(close) :]

    result.append(remaining)
    return "".join(result)


def _convert_single_asterisk_italic(text: str) -> str:
    result: list[str] = []
    chars = list(text)
    i = 0

    while i < len(chars):
        if chars[i] == "*":
            j = i + 1
            while j < len(chars) and chars[j] != "*" and chars[j] != "\n":
                j += 1
            if j < len(chars) and chars[j] == "*" and j > i + 1:
                result.append("_")
                result.extend(chars[i + 1 : j])
                result.append("_")
                i = j + 1
            else:
                result.append(chars[i])
                i += 1
        else:
            result.append(chars[i])
            i += 1

    return "".join(result)
