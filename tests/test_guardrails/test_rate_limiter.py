"""Tests for rate_limiter."""

from oac_slack_bot.guardrails.rate_limiter import RateLimiter


def test_allows_within_limit():
    rl = RateLimiter(user_limit=3, channel_limit=10)
    assert rl.check_user("U1")
    assert rl.check_user("U1")
    assert rl.check_user("U1")


def test_blocks_over_limit():
    rl = RateLimiter(user_limit=2, channel_limit=10)
    assert rl.check_user("U1")
    assert rl.check_user("U1")
    assert not rl.check_user("U1")


def test_separate_users():
    rl = RateLimiter(user_limit=1, channel_limit=10)
    assert rl.check_user("U1")
    assert rl.check_user("U2")  # Different user, OK
    assert not rl.check_user("U1")


def test_channel_limit():
    rl = RateLimiter(user_limit=100, channel_limit=2)
    assert rl.check_channel("C1")
    assert rl.check_channel("C1")
    assert not rl.check_channel("C1")
