"""Plugin registry — scans directories for SKILL.md files."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from pathlib import Path

import structlog

logger = structlog.get_logger()

MAX_DEPTH = 5
SKIP_DIRS = {"node_modules", ".git", "target"}


@dataclass
class Plugin:
    name: str
    group: str | None
    fqn: str
    description: str
    system_prompt: str
    source_path: Path


class GetError(Enum):
    NOT_FOUND = "not_found"
    AMBIGUOUS = "ambiguous"


class PluginRegistry:
    """Registry of plugins loaded from SKILL.md files."""

    def __init__(self, plugins: dict[str, Plugin] | None = None) -> None:
        self._plugins: dict[str, Plugin] = plugins or {}

    @classmethod
    def load(cls, dirs: list[str]) -> PluginRegistry:
        """Scan directories for SKILL.md files and build registry."""
        plugins: dict[str, Plugin] = {}

        for dir_path in dirs:
            path = Path(dir_path)
            if not path.exists():
                logger.debug("plugin_dir_missing", dir=dir_path)
                continue
            if not path.is_dir():
                logger.warning("plugin_dir_not_directory", dir=dir_path)
                continue

            skill_files = _find_skill_files(path, 0)
            logger.debug("found_skill_files", dir=dir_path, count=len(skill_files))

            for skill_path in skill_files:
                plugin = _parse_skill_file(skill_path)
                if plugin is None:
                    continue
                if plugin.name.startswith("_"):
                    logger.debug("skipping_private_plugin", name=plugin.name)
                    continue
                logger.debug("loaded_plugin", fqn=plugin.fqn, path=str(skill_path))
                plugins[plugin.fqn] = plugin

        logger.info("plugin_registry_loaded", count=len(plugins))
        return cls(plugins)

    def get(self, key: str) -> Plugin | None:
        """Look up by FQN first, then try bare name (if unambiguous)."""
        if key in self._plugins:
            return self._plugins[key]

        if ":" in key:
            return None

        matches = [p for p in self._plugins.values() if p.name == key]
        if len(matches) == 1:
            return matches[0]
        if len(matches) > 1:
            fqns = [p.fqn for p in matches]
            logger.debug("ambiguous_bare_name", bare_name=key, candidates=fqns)
        return None

    def get_or_ambiguous(self, key: str) -> Plugin | tuple[GetError, list[str]]:
        """Like get, but returns error info for ambiguous bare names."""
        if key in self._plugins:
            return self._plugins[key]

        if ":" in key:
            return (GetError.NOT_FOUND, [])

        matches = [p for p in self._plugins.values() if p.name == key]
        if len(matches) == 1:
            return matches[0]
        if len(matches) == 0:
            return (GetError.NOT_FOUND, [])
        return (GetError.AMBIGUOUS, [p.fqn for p in matches])

    def list(self) -> list[tuple[str, str, str | None]]:
        """Return (fqn, description, group) sorted by FQN."""
        result = [(p.fqn, p.description, p.group) for p in self._plugins.values()]
        result.sort(key=lambda x: x[0])
        return result

    def get_group_commands(self, group: str) -> list[Plugin]:
        result = [p for p in self._plugins.values() if p.group == group]
        result.sort(key=lambda p: p.fqn)
        return result

    def groups(self) -> list[str]:
        groups = sorted({p.group for p in self._plugins.values() if p.group})
        return groups

    def __len__(self) -> int:
        return len(self._plugins)

    def is_empty(self) -> bool:
        return len(self._plugins) == 0


# ── Helpers ──


def _find_skill_files(directory: Path, depth: int) -> list[Path]:
    if depth > MAX_DEPTH:
        return []

    results: list[Path] = []
    try:
        entries = list(directory.iterdir())
    except OSError as e:
        logger.warning("cannot_read_plugin_dir", dir=str(directory), error=str(e))
        return results

    for entry in entries:
        if entry.is_dir():
            if entry.name in SKIP_DIRS:
                continue
            results.extend(_find_skill_files(entry, depth + 1))
        elif entry.is_file() and entry.name == "SKILL.md":
            results.append(entry)

    return results


def _derive_fqn(path: Path, name: str) -> tuple[str | None, str]:
    """Derive (group, fqn) from SKILL.md path."""
    command_dir = path.parent
    maybe_skills_dir = command_dir.parent

    if maybe_skills_dir.name == "skills":
        group_dir = maybe_skills_dir.parent
        group_name = group_dir.name
        if not group_name.startswith("."):
            return (group_name, f"{group_name}:{name}")

    return (None, name)


def _parse_skill_file(path: Path) -> Plugin | None:
    try:
        content = path.read_text(encoding="utf-8")
    except OSError as e:
        logger.warning("cannot_read_skill", path=str(path), error=str(e))
        return None
    return _parse_skill_content(content, path)


def _parse_skill_content(content: str, path: Path) -> Plugin | None:
    content = content.lstrip("\n")
    if not content.startswith("---"):
        logger.warning("skill_missing_frontmatter", path=str(path))
        return None

    after_first = content[3:]
    closing = after_first.find("\n---")
    if closing < 0:
        return None

    frontmatter = after_first[:closing].strip()
    rest = after_first[closing + 4:]
    system_prompt = rest.strip()

    name = _extract_frontmatter_field(frontmatter, "name")
    if not name:
        logger.warning("skill_empty_name", path=str(path))
        return None

    description = _extract_frontmatter_field(frontmatter, "description") or ""
    group, fqn = _derive_fqn(path, name)

    # Load companion files
    companion_files = _extract_frontmatter_list(frontmatter, "companion-files")
    if companion_files:
        skill_dir = path.parent
        for filename in companion_files:
            companion_path = skill_dir / filename
            try:
                companion_content = companion_path.read_text(encoding="utf-8")
                system_prompt += f"\n\n---\n# {filename}\n\n{companion_content.strip()}"
                logger.debug("loaded_companion_file", fqn=fqn, file=filename)
            except OSError as e:
                logger.warning("companion_file_missing", fqn=fqn, file=filename, error=str(e))

    return Plugin(
        name=name,
        group=group,
        fqn=fqn,
        description=description,
        system_prompt=system_prompt,
        source_path=path,
    )


def _extract_frontmatter_field(frontmatter: str, field: str) -> str | None:
    prefix = f"{field}:"
    for line in frontmatter.splitlines():
        line = line.strip()
        if line.startswith(prefix):
            value = line[len(prefix) :].strip()
            if (value.startswith('"') and value.endswith('"')) or (
                value.startswith("'") and value.endswith("'")
            ):
                value = value[1:-1]
            return value
    return None


def _extract_frontmatter_list(frontmatter: str, field: str) -> list[str]:
    prefix = f"{field}:"
    result: list[str] = []
    in_list = False

    for line in frontmatter.splitlines():
        trimmed = line.strip()

        if trimmed.startswith(prefix):
            after = trimmed[len(prefix) :].strip()
            if not after:
                in_list = True
                continue
            return result  # Inline value, not a list

        if in_list:
            if trimmed.startswith("- "):
                item = trimmed[2:].strip()
                if (item.startswith('"') and item.endswith('"')) or (
                    item.startswith("'") and item.endswith("'")
                ):
                    item = item[1:-1]
                if item:
                    result.append(item)
            elif trimmed:
                break  # Non-list, non-empty line

    return result
