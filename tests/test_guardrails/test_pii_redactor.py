"""Tests for pii_redactor."""

from oac_slack_bot.guardrails.pii_redactor import PIIRedactor


def test_redact_email():
    r = PIIRedactor()
    assert "[REDACTED_EMAIL]" in r.redact("contact user@example.com for help")


def test_redact_api_key():
    r = PIIRedactor()
    assert "[REDACTED_API_KEY]" in r.redact("use key sk-abc123def456ghi789xyz")


def test_redact_slack_token():
    r = PIIRedactor()
    assert "[REDACTED_SLACK_TOKEN]" in r.redact("token xoxb-1234-5678-abcdef")


def test_no_pii():
    r = PIIRedactor()
    text = "just a normal message"
    assert r.redact(text) == text


def test_disabled():
    r = PIIRedactor(enabled=False)
    text = "contact user@example.com"
    assert r.redact(text) == text
