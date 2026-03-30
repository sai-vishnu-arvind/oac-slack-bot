"""Plugin execution via claude CLI subprocess."""

from __future__ import annotations

import asyncio
import json
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

import structlog

from oac_slack_bot.plugins.registry import Plugin

logger = structlog.get_logger()


@dataclass
class CliResult:
    text: str
    session_id: str | None = None


async def execute_plugin_via_cli(
    plugin: Plugin,
    query: str,
    session_id: str | None = None,
    mcp_config_path: str | None = None,
) -> CliResult:
    """Execute a plugin by spawning the claude CLI subprocess."""
    args = [
        "claude",
        "--print",
        "--dangerously-skip-permissions",
        "--output-format", "stream-json",
        "--verbose",
        "--model", "sonnet",
        "--system-prompt", plugin.system_prompt,
    ]

    if mcp_config_path and Path(mcp_config_path).exists():
        args.extend(["--mcp-config", mcp_config_path])

    if session_id:
        args.extend(["--resume", session_id])

    # Extra --print for stdin mode
    args.append("--print")

    logger.info(
        "spawning_claude_cli",
        fqn=plugin.fqn,
        query_len=len(query),
        has_session=session_id is not None,
    )

    proc = await asyncio.create_subprocess_exec(
        *args,
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )

    stdout_bytes, stderr_bytes = await proc.communicate(input=query.encode())
    stdout = stdout_bytes.decode(errors="replace")
    stderr = stderr_bytes.decode(errors="replace")

    if proc.returncode != 0 and not stdout:
        logger.error(
            "claude_cli_error",
            fqn=plugin.fqn,
            exit_code=proc.returncode,
            stderr=stderr[:500],
        )
        raise RuntimeError(
            f"Claude CLI error (exit {proc.returncode}): {stderr[:500]}"
        )

    # Parse stream-json output
    text = ""
    cli_session_id: str | None = None
    cost: float = 0.0
    duration_ms: int = 0

    for line in stdout.splitlines():
        if not line.strip():
            continue
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError:
            continue

        msg_type = parsed.get("type", "")

        if msg_type == "assistant":
            content = (parsed.get("message") or {}).get("content", [])
            if isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and "text" in block:
                        text += block["text"]

        elif msg_type == "result":
            result_text = parsed.get("result", "")
            if result_text and not text:
                text = result_text
            cli_session_id = parsed.get("session_id")
            cost = parsed.get("total_cost_usd", 0.0)
            duration_ms = parsed.get("duration_ms", 0)

    logger.info(
        "claude_cli_complete",
        fqn=plugin.fqn,
        text_len=len(text),
        session_id=cli_session_id,
        cost_usd=cost,
        duration_ms=duration_ms,
    )

    return CliResult(text=text, session_id=cli_session_id)


async def execute_plugin_via_cli_streaming(
    plugin: Plugin,
    query: str,
    session_id: str | None = None,
    mcp_config_path: str | None = None,
    on_text: Callable[[str], None] | None = None,
) -> CliResult:
    """Execute a plugin with streaming output, calling on_text for each chunk."""
    args = [
        "claude",
        "--print",
        "--dangerously-skip-permissions",
        "--output-format", "stream-json",
        "--verbose",
        "--model", "sonnet",
        "--system-prompt", plugin.system_prompt,
    ]

    if mcp_config_path and Path(mcp_config_path).exists():
        args.extend(["--mcp-config", mcp_config_path])

    if session_id:
        args.extend(["--resume", session_id])

    logger.info(
        "spawning_claude_cli_streaming",
        fqn=plugin.fqn,
        query_len=len(query),
        has_session=session_id is not None,
    )

    proc = await asyncio.create_subprocess_exec(
        *args,
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )

    # Write query to stdin and close
    if proc.stdin:
        proc.stdin.write(query.encode())
        await proc.stdin.drain()
        proc.stdin.close()

    full_text = ""
    cli_session_id: str | None = None

    # Read stdout line by line
    if proc.stdout:
        while True:
            line_bytes = await proc.stdout.readline()
            if not line_bytes:
                break
            line = line_bytes.decode(errors="replace").strip()
            if not line:
                continue

            try:
                parsed = json.loads(line)
            except json.JSONDecodeError:
                continue

            msg_type = parsed.get("type", "")

            if msg_type == "assistant":
                message = parsed.get("message", {})
                content = message.get("content", [])
                if isinstance(content, list):
                    for block in content:
                        if isinstance(block, dict) and "text" in block:
                            chunk = block["text"]
                            full_text += chunk
                            if on_text:
                                on_text(chunk)

            elif msg_type == "result":
                result_text = parsed.get("result", "")
                if result_text:
                    full_text = result_text
                cli_session_id = parsed.get("session_id")
                cost = parsed.get("total_cost_usd", 0.0)
                logger.info(
                    "claude_cli_streaming_complete",
                    fqn=plugin.fqn,
                    text_len=len(full_text),
                    session_id=cli_session_id,
                    cost_usd=cost,
                )

            elif msg_type == "system":
                subtype = parsed.get("subtype", "")
                if subtype == "init":
                    cli_session_id = parsed.get("session_id")

    await proc.wait()

    if proc.returncode and proc.returncode != 0:
        logger.warning(
            "claude_cli_nonzero_exit",
            fqn=plugin.fqn,
            exit_code=proc.returncode,
        )

    return CliResult(text=full_text, session_id=cli_session_id)


async def execute_plugin(
    plugin: Plugin,
    query: str,
) -> str:
    """Backward-compatible wrapper for execute_plugin_via_cli."""
    mcp_config = None
    try:
        candidate = Path.cwd() / "mcp-servers.json"
        if candidate.exists():
            mcp_config = str(candidate)
    except OSError:
        pass

    result = await execute_plugin_via_cli(
        plugin, query, session_id=None, mcp_config_path=mcp_config
    )
    return result.text
