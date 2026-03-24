#!/usr/bin/env node
/**
 * acteon-swarm Agent SDK bridge.
 *
 * Thin wrapper around @anthropic-ai/claude-agent-sdk that:
 * - Accepts prompt, system prompt, allowed tools, and working directory via CLI args
 * - Streams NDJSON messages to stdout (text, tool_use, result, error)
 * - Uses the user's existing Claude Code authentication (no API keys)
 *
 * The Rust orchestrator spawns this as a subprocess and reads NDJSON lines.
 */

import { query } from "@anthropic-ai/claude-agent-sdk";
import { parseArgs } from "node:util";

const { values: args } = parseArgs({
  options: {
    prompt: { type: "string" },
    "system-prompt": { type: "string", default: "" },
    "allowed-tools": { type: "string", default: "" },
    cwd: { type: "string", default: process.cwd() },
  },
  strict: true,
});

if (!args.prompt) {
  process.stderr.write("error: --prompt is required\n");
  process.exit(1);
}

/** Write a single NDJSON line to stdout. */
function emit(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

try {
  const options = {
    cwd: args.cwd,
  };

  if (args["allowed-tools"]) {
    options.allowedTools = args["allowed-tools"].split(",").filter(Boolean);
  }

  if (args["system-prompt"]) {
    options.systemPrompt = args["system-prompt"];
  }

  let finalResult = "";
  let sessionId = "";

  for await (const message of query({
    prompt: args.prompt,
    options,
  })) {
    if (message.type === "assistant") {
      // Assistant message with content blocks.
      for (const block of message.message?.content ?? []) {
        if (block.type === "text") {
          emit({ type: "text", content: block.text });
        } else if (block.type === "tool_use") {
          emit({ type: "tool_use", tool: block.name, input: block.input });
        }
      }
    } else if (message.type === "result") {
      finalResult = message.result ?? "";
      sessionId = message.session_id ?? "";
    }
  }

  emit({ type: "result", content: finalResult, session_id: sessionId });
  process.exit(0);
} catch (err) {
  emit({ type: "error", message: err.message ?? String(err) });
  process.exit(1);
}
