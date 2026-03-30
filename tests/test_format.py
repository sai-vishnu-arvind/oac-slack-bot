"""Tests for slack/format.py — ported from Rust tests."""

from oac_slack_bot.slack.format import md_to_slack


# ── Headings ──

def test_heading_h1():
    assert md_to_slack("# Hello") == "\n*Hello*"

def test_heading_h3():
    assert md_to_slack("### Sub-heading") == "\n*Sub-heading*"

def test_heading_not_a_heading():
    assert md_to_slack("#hashtag") == "#hashtag"


# ── Bold / italic / strikethrough ──

def test_bold_double_asterisk():
    assert md_to_slack("**bold**") == "*bold*"

def test_bold_double_underscore():
    assert md_to_slack("__bold__") == "*bold*"

def test_italic_single_asterisk():
    assert md_to_slack("*italic*") == "_italic_"

def test_bold_and_italic_in_same_line():
    assert md_to_slack("**bold** and *italic*") == "*bold* and _italic_"

def test_bold_italic_triple_asterisk():
    assert md_to_slack("***both***") == "*_both_*"

def test_strikethrough():
    assert md_to_slack("~~deleted~~") == "~deleted~"


# ── Links ──

def test_markdown_link():
    assert md_to_slack("[click here](https://example.com)") == "<https://example.com|click here>"

def test_image_link():
    assert md_to_slack("![logo](https://img.example.com/logo.png)") == "<https://img.example.com/logo.png>"


# ── Lists ──

def test_unordered_list():
    inp = "- first\n- second\n- third"
    expected = "•  first\n•  second\n•  third"
    assert md_to_slack(inp) == expected

def test_ordered_list_preserved():
    inp = "1. first\n2. second"
    assert md_to_slack(inp) == inp


# ── Horizontal rules ──

def test_horizontal_rule_dashes():
    assert md_to_slack("---") == "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

def test_horizontal_rule_asterisks():
    assert md_to_slack("***") == "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"


# ── Code blocks ──

def test_code_block_preserved():
    inp = "```rust\nlet x = **not bold**;\n```"
    assert md_to_slack(inp) == inp

def test_inline_code_preserved():
    assert md_to_slack("use `**raw**` in code") == "use `**raw**` in code"


# ── Block quotes ──

def test_blockquote_passthrough():
    assert md_to_slack("> a quote") == "> a quote"


# ── Mixed content ──

def test_full_message():
    inp = (
        "# Summary\n\n"
        "Here is **bold** and *italic* text.\n\n"
        "- Item one\n"
        "- Item two with [a link](https://example.com)\n\n"
        "---\n\n"
        "> Quote block\n\n"
        "```\ncode block\n```"
    )
    output = md_to_slack(inp)
    assert "*Summary*" in output
    assert "*bold*" in output
    assert "_italic_" in output
    assert "•  Item one" in output
    assert "<https://example.com|a link>" in output
    assert "━━━" in output
    assert "> Quote block" in output
    assert "code block" in output


# ── Edge cases ──

def test_empty_input():
    assert md_to_slack("") == ""

def test_unmatched_bold_marker():
    assert md_to_slack("**unfinished") == "**unfinished"

def test_unmatched_backtick():
    assert md_to_slack("`unfinished code") == "`unfinished code"

def test_multiple_bold_phrases():
    assert md_to_slack("**a** then **b**") == "*a* then *b*"

def test_link_with_bold_text():
    assert md_to_slack("[**bold link**](https://x.com)") == "<https://x.com|*bold link*>"

def test_heading_with_inline_formatting():
    assert md_to_slack("## A **bold** heading") == "\n*A *bold* heading*"

def test_nested_list_indentation():
    output = md_to_slack("- outer\n  - inner")
    assert "•  outer" in output
    assert "•  inner" in output


# ── Tables ──

def test_simple_table():
    inp = "| Name | Value |\n|------|-------|\n| foo  | 123   |\n| bar  | 456   |"
    output = md_to_slack(inp)
    assert "```" in output
    assert "foo" in output
    assert "bar" in output
    assert "|---" not in output

def test_table_surrounded_by_text():
    inp = "Some text before.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nSome text after."
    output = md_to_slack(inp)
    assert "Some text before." in output
    assert "```" in output
    assert "Some text after." in output

def test_not_a_table_just_pipes():
    output = md_to_slack("use | for OR")
    assert "```" not in output


# ── Inline formatting edge cases ──

def test_bold_inside_list_item():
    output = md_to_slack("- **Key**: value")
    assert "•  *Key*: value" in output

def test_link_inside_list_item():
    output = md_to_slack("- See [docs](https://x.com)")
    assert output == "•  See <https://x.com|docs>"

def test_table_inside_code_block_not_converted():
    inp = "```\n| A | B |\n|---|---|\n| 1 | 2 |\n```"
    output = md_to_slack(inp)
    assert "| A | B |" in output
    assert "--+--" not in output

def test_multiple_code_blocks():
    inp = "```\na\n```\n**bold**\n```\nb\n```"
    output = md_to_slack(inp)
    assert "*bold*" in output

def test_inline_code_next_to_bold():
    output = md_to_slack("`code` and **bold**")
    assert "`code`" in output
    assert "*bold*" in output

def test_link_with_query_params():
    output = md_to_slack("[click](https://x.com/a?b=1&c=2)")
    assert "<https://x.com/a?b=1&c=2|click>" in output

def test_blockquote_with_bold():
    output = md_to_slack("> **important** note")
    assert "> *important* note" in output

def test_bold_markers_inside_table_stripped():
    inp = "| **Name** | Value |\n|---|---|\n| foo | bar |"
    output = md_to_slack(inp)
    assert "```" in output
    assert "Name" in output
    assert "**Name**" not in output

def test_heading_then_table():
    inp = "# Title\n| A | B |\n|---|---|\n| 1 | 2 |"
    output = md_to_slack(inp)
    assert "*Title*" in output
    assert "```" in output

def test_numbered_list_with_bold():
    output = md_to_slack("1. **First** item\n2. Second")
    assert "*First*" in output
    assert "1." in output


def test_realistic_claude_output():
    inp = (
        "## Root Cause Analysis\n\n"
        "The issue is caused by **wrong identifier** passed to the API.\n\n"
        "| Field | Failure | Success |\n"
        "|-------|---------|--------|\n"
        "| ID    | abc     | xyz     |\n\n"
        "### Recommendations\n\n"
        "1. **Fix the identifier** — use `mihpayid` instead of `bank_ref_num`\n"
        "2. Add validation to catch this early\n"
        "- Check the [API docs](https://docs.example.com)\n\n"
        "> Note: This is a known issue.\n\n"
        '```json\n{"var1": "correct_value"}\n```'
    )
    output = md_to_slack(inp)

    assert "*Root Cause Analysis*" in output
    assert "*Recommendations*" in output
    assert "*wrong identifier*" in output
    assert "*Fix the identifier*" in output
    assert "```\n" in output
    assert "Failure" in output
    assert "|---" not in output
    assert "`mihpayid`" in output
    assert "<https://docs.example.com|API docs>" in output
    assert "> Note:" in output
    assert '"var1"' in output
