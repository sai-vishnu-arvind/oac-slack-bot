"""Tests for slack/events.py helpers."""

from oac_slack_bot.slack.events import (
    build_thread_context,
    extract_devrev_ticket_ids,
    extract_slack_urls,
    merge_resolved,
    strip_mentions,
)
from oac_slack_bot.slack.types import ThreadMessage


def test_strip_mentions():
    assert strip_mentions("<@U12345> hello") == "hello"
    assert strip_mentions("hi <@U12345> there") == "hi there"
    assert strip_mentions("no mention") == "no mention"


def test_extract_slack_urls():
    msg = "check https://razorpay.slack.com/archives/C12345/p1700000000123456"
    urls = extract_slack_urls(msg)
    assert len(urls) == 1
    assert urls[0] == ("C12345", "1700000000.123456")


def test_extract_slack_urls_with_angle_brackets():
    msg = "see <https://razorpay.slack.com/archives/C99/p1234567890123456>"
    urls = extract_slack_urls(msg)
    assert len(urls) == 1
    assert urls[0][0] == "C99"


def test_extract_slack_urls_no_match():
    assert extract_slack_urls("just text") == []


def test_extract_devrev_ticket_ids():
    ids = extract_devrev_ticket_ids("look at TKT-12345 and ISS-67890")
    assert "TKT-12345" in ids
    assert "ISS-67890" in ids


def test_extract_devrev_from_url():
    msg = "see https://devrev.ai/works/TKT-999"
    ids = extract_devrev_ticket_ids(msg)
    assert "TKT-999" in ids


def test_build_thread_context():
    msgs = [
        ThreadMessage(user="U1", text="hello", ts="1.0"),
        ThreadMessage(user=None, text="bot response", ts="2.0", bot_id="B1"),
        ThreadMessage(user="U2", text="thanks", ts="3.0"),
    ]
    ctx = build_thread_context(msgs, exclude_ts="3.0")
    assert "[U1]: hello" in ctx
    assert "[assistant]: bot response" in ctx
    assert "thanks" not in ctx


def test_build_thread_context_skips_indicators():
    msgs = [
        ThreadMessage(user=None, text="⏳ Thinking...", ts="1.0", bot_id="B1"),
        ThreadMessage(user=None, text="🔍 Running plugin: `foo`...", ts="2.0", bot_id="B1"),
        ThreadMessage(user="U1", text="real message", ts="3.0"),
    ]
    ctx = build_thread_context(msgs)
    assert "Thinking" not in ctx
    assert "Running plugin" not in ctx
    assert "real message" in ctx


def test_merge_resolved_neither():
    assert merge_resolved("orig", "orig", "orig") == "orig"


def test_merge_resolved_slack_only():
    slack = "ctx\n\nUser's question: orig"
    assert merge_resolved("orig", slack, "orig") == slack


def test_merge_resolved_both():
    slack = "slack-ctx\n\nUser's question: orig"
    devrev = "devrev-ctx\n\nUser's question: orig"
    result = merge_resolved("orig", slack, devrev)
    assert "slack-ctx" in result
    assert "devrev-ctx" in result
    assert "User's question: orig" in result
