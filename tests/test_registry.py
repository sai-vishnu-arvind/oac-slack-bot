"""Tests for plugins/registry.py — ported from Rust tests."""

import os
from pathlib import Path

import pytest

from oac_slack_bot.plugins.registry import (
    GetError,
    PluginRegistry,
    _derive_fqn,
    _extract_frontmatter_field,
    _extract_frontmatter_list,
    _parse_skill_content,
)


def _make_skill(base: Path, name: str, description: str, body: str) -> None:
    d = base / name
    d.mkdir(parents=True, exist_ok=True)
    (d / "SKILL.md").write_text(f"---\nname: {name}\ndescription: {description}\n---\n\n{body}")


def _make_grouped_skill(base: Path, group: str, cmd: str, desc: str, body: str) -> None:
    d = base / group / "skills" / cmd
    d.mkdir(parents=True, exist_ok=True)
    (d / "SKILL.md").write_text(f"---\nname: {cmd}\ndescription: {desc}\n---\n\n{body}")


def test_load_empty():
    reg = PluginRegistry.load([])
    assert len(reg) == 0


def test_load_single(tmp_path: Path):
    _make_skill(tmp_path, "my-plugin", "Does something", "You are a helper.")
    reg = PluginRegistry.load([str(tmp_path)])
    assert len(reg) == 1
    p = reg.get("my-plugin")
    assert p is not None
    assert p.name == "my-plugin"
    assert p.fqn == "my-plugin"
    assert p.group is None
    assert p.description == "Does something"


def test_skip_underscore(tmp_path: Path):
    _make_skill(tmp_path, "_private", "Internal", "secret")
    _make_skill(tmp_path, "public", "Public", "public")
    reg = PluginRegistry.load([str(tmp_path)])
    assert len(reg) == 1
    assert reg.get("_private") is None
    assert reg.get("public") is not None


def test_later_dir_overrides(tmp_path: Path):
    d1 = tmp_path / "d1"
    d2 = tmp_path / "d2"
    d1.mkdir()
    d2.mkdir()
    _make_skill(d1, "shared", "V1", "Prompt v1")
    _make_skill(d2, "shared", "V2", "Prompt v2")
    reg = PluginRegistry.load([str(d1), str(d2)])
    assert len(reg) == 1
    assert reg.get("shared").description == "V2"


def test_derive_fqn_nested():
    path = Path("/home/user/plugins/second-brain/skills/capture/SKILL.md")
    group, fqn = _derive_fqn(path, "capture")
    assert group == "second-brain"
    assert fqn == "second-brain:capture"


def test_derive_fqn_flat():
    path = Path("/home/user/.agents/skills/oncall-debugger/SKILL.md")
    group, fqn = _derive_fqn(path, "oncall-debugger")
    assert group is None
    assert fqn == "oncall-debugger"


def test_grouped_plugin(tmp_path: Path):
    _make_grouped_skill(tmp_path, "second-brain", "capture", "Captures", "prompt")
    reg = PluginRegistry.load([str(tmp_path)])
    assert len(reg) == 1
    p = reg.get("second-brain:capture")
    assert p is not None
    assert p.group == "second-brain"
    # Bare name lookup (unambiguous)
    assert reg.get("capture") is not None


def test_ambiguous_bare_name(tmp_path: Path):
    _make_grouped_skill(tmp_path, "group-a", "init", "A", "p1")
    _make_grouped_skill(tmp_path, "group-b", "init", "B", "p2")
    reg = PluginRegistry.load([str(tmp_path)])
    assert len(reg) == 2
    assert reg.get("group-a:init") is not None
    assert reg.get("group-b:init") is not None
    assert reg.get("init") is None  # Ambiguous


def test_get_or_ambiguous(tmp_path: Path):
    _make_grouped_skill(tmp_path, "group-a", "init", "A", "p1")
    _make_grouped_skill(tmp_path, "group-b", "init", "B", "p2")
    reg = PluginRegistry.load([str(tmp_path)])

    assert isinstance(reg.get_or_ambiguous("group-a:init"), object)
    result = reg.get_or_ambiguous("init")
    assert isinstance(result, tuple)
    assert result[0] == GetError.AMBIGUOUS
    assert len(result[1]) == 2


def test_groups(tmp_path: Path):
    _make_grouped_skill(tmp_path, "second-brain", "capture", "Cap", "p1")
    _make_grouped_skill(tmp_path, "agent-ready", "init", "Init", "p2")
    _make_skill(tmp_path, "flat", "Flat", "p3")
    reg = PluginRegistry.load([str(tmp_path)])
    assert reg.groups() == ["agent-ready", "second-brain"]


def test_companion_files(tmp_path: Path):
    d = tmp_path / "my-plugin"
    d.mkdir()
    (d / "SKILL.md").write_text(
        "---\nname: my-plugin\ndescription: Test\ncompanion-files:\n  - TREE.md\n  - PROMPT.md\n---\n\nBase prompt."
    )
    (d / "TREE.md").write_text("# Decision Tree\nClassify here.")
    (d / "PROMPT.md").write_text("# System Prompt\nYou are a bot.")

    reg = PluginRegistry.load([str(tmp_path)])
    p = reg.get("my-plugin")
    assert "Base prompt." in p.system_prompt
    assert "# TREE.md" in p.system_prompt
    assert "Classify here." in p.system_prompt
    assert "# PROMPT.md" in p.system_prompt
    assert "You are a bot." in p.system_prompt


def test_extract_frontmatter_list():
    fm = "name: test\ncompanion-files:\n  - A.md\n  - B.md\n  - C.md"
    assert _extract_frontmatter_list(fm, "companion-files") == ["A.md", "B.md", "C.md"]


def test_extract_frontmatter_list_quoted():
    fm = 'name: test\ncompanion-files:\n  - "A.md"\n  - \'B.md\''
    assert _extract_frontmatter_list(fm, "companion-files") == ["A.md", "B.md"]
