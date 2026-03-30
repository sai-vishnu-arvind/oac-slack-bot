"""Tests for plugins/router.py — ported from Rust tests."""

from pathlib import Path

from oac_slack_bot.plugins.registry import PluginRegistry
from oac_slack_bot.plugins.router import route


def _make_skill(base: Path, name: str, desc: str, body: str) -> None:
    d = base / name
    d.mkdir(parents=True, exist_ok=True)
    (d / "SKILL.md").write_text(f"---\nname: {name}\ndescription: {desc}\n---\n\n{body}")


def _make_grouped(base: Path, group: str, cmd: str, desc: str, body: str) -> None:
    d = base / group / "skills" / cmd
    d.mkdir(parents=True, exist_ok=True)
    (d / "SKILL.md").write_text(f"---\nname: {cmd}\ndescription: {desc}\n---\n\n{body}")


def _build(tmp_path: Path, plugins: list[tuple[str, str, str]]) -> PluginRegistry:
    for name, desc, body in plugins:
        _make_skill(tmp_path, name, desc, body)
    return PluginRegistry.load([str(tmp_path)])


def test_firing_pattern(tmp_path: Path):
    reg = _build(tmp_path, [
        ("oncall-debugger", "Debugs oncall incidents", "prompt"),
        ("other-plugin", "Unrelated", "prompt"),
    ])
    p = route("[FIRING] Alert: high error rate", reg)
    assert p is not None
    assert p.fqn == "oncall-debugger"


def test_firing_fallback(tmp_path: Path):
    reg = _build(tmp_path, [
        ("systematic-solver-v2", "Systematic solver", "prompt"),
    ])
    p = route("[CRITICAL] Database down", reg)
    assert p is not None
    assert p.fqn == "systematic-solver-v2"


def test_exact_name_match(tmp_path: Path):
    reg = _build(tmp_path, [
        ("nexus-platform-faq", "Nexus FAQ", "prompt"),
        ("other-plugin", "Unrelated", "prompt"),
    ])
    p = route("Can you help with nexus-platform-faq?", reg)
    assert p is not None
    assert p.fqn == "nexus-platform-faq"


def test_force_override(tmp_path: Path):
    reg = _build(tmp_path, [
        ("nexus-platform-faq", "FAQ", "prompt"),
        ("other-plugin", "Other", "prompt"),
    ])
    p = route("use /plugin other-plugin for this", reg)
    assert p is not None
    assert p.fqn == "other-plugin"


def test_no_match_returns_none(tmp_path: Path):
    reg = _build(tmp_path, [("very-specific", "Very specific thing", "prompt")])
    assert route("hi how are you", reg) is None


def test_description_keyword_match(tmp_path: Path):
    reg = _build(tmp_path, [
        ("billing-helper", "Assists with invoice and payment questions", "prompt"),
    ])
    p = route("I have a question about my invoice", reg)
    assert p is not None
    assert p.fqn == "billing-helper"


def test_fqn_in_message(tmp_path: Path):
    _make_grouped(tmp_path, "second-brain", "capture", "Captures", "prompt")
    reg = PluginRegistry.load([str(tmp_path)])
    p = route("run second-brain:capture on this", reg)
    assert p is not None
    assert p.fqn == "second-brain:capture"


def test_fqn_resolves_ambiguity(tmp_path: Path):
    _make_grouped(tmp_path, "agent-ready", "init", "AR init", "p1")
    _make_grouped(tmp_path, "fe-agent-ready", "init", "FE init", "p2")
    reg = PluginRegistry.load([str(tmp_path)])

    p1 = route("run agent-ready:init", reg)
    assert p1 is not None
    assert p1.fqn == "agent-ready:init"

    p2 = route("run fe-agent-ready:init", reg)
    assert p2 is not None
    assert p2.fqn == "fe-agent-ready:init"


def test_ambiguous_bare_name_no_match(tmp_path: Path):
    _make_grouped(tmp_path, "agent-ready", "init", "Sets up AR", "p1")
    _make_grouped(tmp_path, "fe-agent-ready", "init", "Sets up FE", "p2")
    reg = PluginRegistry.load([str(tmp_path)])
    assert route("run init", reg) is None
