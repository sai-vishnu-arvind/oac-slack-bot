"""Tests for input_validator."""

from oac_slack_bot.guardrails.input_validator import InputValidator


def test_valid_message():
    v = InputValidator(max_length=100)
    assert v.validate("hello world").allowed


def test_too_long():
    v = InputValidator(max_length=10)
    assert not v.validate("a" * 20).allowed


def test_null_bytes():
    v = InputValidator()
    assert not v.validate("hello\x00world").allowed


def test_mostly_whitespace():
    v = InputValidator()
    text = "x" + " " * 200
    assert not v.validate(text).allowed
