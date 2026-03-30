"""Tests for metrics.py — ported from Rust tests."""

import pytest

from oac_slack_bot.metrics import Metrics, _format_ms, _format_number, _percentiles, _top_n


@pytest.mark.asyncio
async def test_record_mention():
    m = Metrics()
    await m.record_mention("U123", "C456")
    await m.record_mention("U123", "C456")
    await m.record_mention("U789", "C456")
    assert m.total_mentions == 3
    assert m._user_calls["U123"] == 2
    assert m._user_calls["U789"] == 1
    assert m._channel_calls["C456"] == 3


@pytest.mark.asyncio
async def test_record_reaction_positive():
    m = Metrics()
    await m.register_bot_message("C1", "1.0", "1.0", "plugin-a", "U1", "how?")
    info = await m.record_reaction("C1", "1.0", "+1", "U2")
    assert info is not None
    assert m.thumbs_up == 1
    assert m.thumbs_down == 0


@pytest.mark.asyncio
async def test_record_reaction_negative():
    m = Metrics()
    await m.register_bot_message("C1", "2.0", "2.0", None, None, "what?")
    info = await m.record_reaction("C1", "2.0", "-1", "U3")
    assert info is not None
    assert m.thumbs_down == 1


@pytest.mark.asyncio
async def test_record_reaction_untracked():
    m = Metrics()
    info = await m.record_reaction("C1", "999.0", "+1", "U1")
    assert info is None
    assert m.thumbs_up == 0


@pytest.mark.asyncio
async def test_record_reaction_irrelevant_emoji():
    m = Metrics()
    await m.register_bot_message("C1", "1.0", "1.0", None, None, "q")
    info = await m.record_reaction("C1", "1.0", "eyes", "U1")
    assert info is None


@pytest.mark.asyncio
async def test_plugin_call_counting():
    m = Metrics()
    await m.record_plugin_call("oncall-debugger")
    await m.record_plugin_call("oncall-debugger")
    await m.record_plugin_call("backend-eng")
    assert m._plugin_calls["oncall-debugger"] == 2
    assert m._plugin_calls["backend-eng"] == 1


def test_percentiles_empty():
    assert _percentiles([]) == (0, 0)

def test_percentiles_single():
    assert _percentiles([100]) == (100, 100)

def test_percentiles_normal():
    vals = list(range(1, 101))
    avg, p95 = _percentiles(vals)
    assert avg == 50
    assert p95 == 96


def test_top_n():
    m = {"a": 10, "b": 30, "c": 20}
    top = _top_n(m, 2)
    assert len(top) == 2
    assert top[0] == ("b", 30)
    assert top[1] == ("c", 20)


def test_format_ms():
    assert _format_ms(0) == "—"
    assert _format_ms(500) == "500ms"
    assert _format_ms(3200) == "3.2s"


def test_format_number():
    assert _format_number(0) == "0"
    assert _format_number(999) == "999"
    assert _format_number(1234) == "1,234"
    assert _format_number(1234567) == "1,234,567"


@pytest.mark.asyncio
async def test_format_stats_output():
    m = Metrics()
    await m.record_mention("U1", "C1")
    stats = await m.format_stats()
    assert "📊 *Bot Stats*" in stats
    assert "Mentions: 1" in stats
