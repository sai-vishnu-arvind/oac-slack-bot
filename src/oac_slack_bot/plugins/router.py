"""5-tier plugin routing."""

from __future__ import annotations

from oac_slack_bot.plugins.registry import Plugin, PluginRegistry


def route(message: str, registry: PluginRegistry) -> Plugin | None:
    """Route a user message to the best matching plugin (if any)."""
    message_lower = message.lower()

    # 1. Force override: /plugin <name>
    plugin = _try_force_override(message, registry)
    if plugin:
        return plugin

    # 2. OAC triage patterns
    plugin = _try_triage_patterns(message_lower, registry)
    if plugin:
        return plugin

    # 3. FQN match
    plugin = _try_fqn_match(message_lower, registry)
    if plugin:
        return plugin

    # 4. Name keyword match
    plugin = _try_name_keyword_match(message_lower, registry)
    if plugin:
        return plugin

    # 5. Description keyword match
    plugin = _try_description_keyword_match(message_lower, registry)
    if plugin:
        return plugin

    return None


def _clean_word(word: str) -> str:
    """Strip leading/trailing punctuation except -, _, :."""
    return word.strip("".join(c for c in "!@#$%^&*()=+[]{}\\|;'\",.<>?/" if c not in "-_:"))


def _try_force_override(message: str, registry: PluginRegistry) -> Plugin | None:
    marker = "/plugin "
    pos = message.find(marker)
    if pos < 0:
        return None
    after = message[pos + len(marker) :]
    name = after.split()[0] if after.split() else None
    if name:
        return registry.get(name)
    return None


def _try_triage_patterns(message_lower: str, registry: PluginRegistry) -> Plugin | None:
    is_triage = any(
        kw in message_lower
        for kw in ("[firing]", "[critical]", "triage", "diagnose")
    )
    if not is_triage:
        return None

    return registry.get("oncall-debugger") or registry.get("systematic-solver-v2")


def _try_fqn_match(message_lower: str, registry: PluginRegistry) -> Plugin | None:
    words = [_clean_word(w) for w in message_lower.split()]
    for word in words:
        if ":" in word:
            plugin = registry.get(word)
            if plugin:
                return plugin
    return None


def _try_name_keyword_match(message_lower: str, registry: PluginRegistry) -> Plugin | None:
    words = [_clean_word(w) for w in message_lower.split()]

    # Check FQN match first
    for fqn, _, _ in registry.list():
        fqn_lower = fqn.lower()
        if fqn_lower in words:
            return registry.get(fqn)

    # Try bare name
    tried: set[str] = set()
    for fqn, _, _ in registry.list():
        bare = fqn.split(":", 1)[1] if ":" in fqn else fqn
        bare_lower = bare.lower()
        if bare_lower in tried:
            continue
        tried.add(bare_lower)
        if bare_lower in words:
            plugin = registry.get(bare_lower)
            if plugin:
                return plugin

    return None


def _try_description_keyword_match(
    message_lower: str, registry: PluginRegistry
) -> Plugin | None:
    significant = [w for w in message_lower.split() if len(w) > 4]
    if not significant:
        return None

    for fqn, description, _ in registry.list():
        desc_lower = description.lower()
        if any(w in desc_lower for w in significant):
            return registry.get(fqn)

    return None
