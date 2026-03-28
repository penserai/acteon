#!/usr/bin/env python3.10
"""
acteon-swarm Agent SDK bridge (Python).

Thin wrapper around claude-code-sdk that:
- Accepts prompt, system prompt, allowed tools, and working directory via CLI args
- Streams NDJSON messages to stdout (text, tool_use, result, error)
- Uses the user's existing Claude Code authentication (no API keys)

The Rust orchestrator spawns this as a subprocess and reads NDJSON lines.
"""

import argparse
import asyncio
import json
import sys

from claude_code_sdk import query, ClaudeCodeOptions, AssistantMessage, ResultMessage, SystemMessage, TextBlock, ToolUseBlock


def emit(obj: dict) -> None:
    """Write a single NDJSON line to stdout."""
    print(json.dumps(obj), flush=True)


async def run_agent(args) -> None:
    options = ClaudeCodeOptions(cwd=args.cwd)

    if args.allowed_tools:
        options.allowed_tools = [t.strip() for t in args.allowed_tools.split(",") if t.strip()]

    if args.system_prompt:
        options.system_prompt = args.system_prompt

    if args.model:
        options.model = args.model

    if args.max_turns:
        options.max_turns = args.max_turns

    final_result = ""
    session_id = ""

    stream = query(prompt=args.prompt, options=options)

    while True:
        try:
            message = await stream.__anext__()
        except StopAsyncIteration:
            break
        except Exception as e:
            if "Unknown message type" in str(e):
                continue
            raise

        try:
            if isinstance(message, SystemMessage):
                # Extract session_id from init message.
                data = getattr(message, "data", {}) or {}
                if isinstance(data, dict):
                    session_id = data.get("session_id", session_id)

            elif isinstance(message, AssistantMessage):
                content_blocks = getattr(message, "content", []) or []
                for block in content_blocks:
                    if isinstance(block, TextBlock):
                        text = block.text
                        emit({"type": "text", "content": text})
                        final_result = text
                    elif isinstance(block, ToolUseBlock):
                        emit({
                            "type": "tool_use",
                            "tool": block.name,
                            "input": block.input if isinstance(block.input, dict) else {},
                        })

            elif isinstance(message, ResultMessage):
                final_result = getattr(message, "result", final_result) or final_result
                session_id = getattr(message, "session_id", session_id) or session_id

        except Exception:
            pass

    emit({"type": "result", "content": final_result, "session_id": session_id})


async def main() -> None:
    parser = argparse.ArgumentParser(description="acteon-swarm Agent SDK bridge")
    parser.add_argument("--prompt", required=True, help="The prompt to send")
    parser.add_argument("--system-prompt", default="", help="System prompt")
    parser.add_argument("--allowed-tools", default="", help="Comma-separated tool names")
    parser.add_argument("--cwd", default=".", help="Working directory")
    parser.add_argument("--model", default="sonnet", help="Model to use")
    parser.add_argument("--max-turns", type=int, default=30, help="Max conversation turns")
    args = parser.parse_args()

    try:
        await run_agent(args)
    except Exception as e:
        emit({"type": "error", "message": str(e)})
        sys.exit(1)


if __name__ == "__main__":
    asyncio.run(main())
