# Acteon Operations Interface Implementation Plan

This document outlines the step-by-step implementation of the Acteon Common Operations Layer, CLI, and MCP server.

## Phase 0: Common Operations Layer (`crates/ops`)
- [ ] **Initialize `crates/ops`**: This crate will wrap `acteon-client` and provide high-level "business operations" (e.g., `dispatch_with_retry`, `search_and_summarize_audit`, `run_simulation`).
- [ ] **Implement Core Command Logic**:
    - Unified configuration handling (API keys, endpoints).
    - Standardized output formatting (JSON for machines, Table/Plain text for humans).

## Phase 1: Acteon CLI (`crates/cli`)
- [ ] **Scaffold CLI**: Use `clap` for command-line argument parsing.
- [ ] **Implement Primary Commands**:
    - `dispatch`: Send events.
    - `audit`: Search and view logs.
    - `rules`: List and simulate rules.
    - `events`: Manage state (ack/resolve).
- [ ] **Interactive Mode**: Add `acteon shell` for interactive exploration.

## Phase 2: MCP Server (`crates/mcp-server`)
- [ ] **Scaffold MCP Server**: Initialize with `mcp-sdk-rs`.
- [ ] **Bridge Ops to MCP**: Map `crates/ops` functions directly to MCP Tools.
- [ ] **Resource Providers**: Map `crates/ops` data fetchers to MCP Resources (`audit://`, `state://`).

## Phase 3: State & Management
- [ ] **`manage_event` tool**:
    - Support `acknowledge` and `resolve` actions.
    - Integrate with `GroupManager` or `StateManager`.
- [ ] **`state://` resource**:
    - Expose current state machine status for a fingerprint.
- [ ] **`list_rules` and `update_rule` tools**:
    - Interface with the rule management system.
    - Implement validation for rule updates.

## Phase 4: Advanced Features
- [ ] **`simulate_rule` tool**:
    - Integrate with `acteon-simulation`.
    - Allow running small-scale simulations in-memory.
- [ ] **`evaluate_policy` tool**:
    - Integrate with `acteon-llm`.
- [ ] **Recurring Actions**:
    - Tools to list, create, and delete `RecurringAction` entries.
- [ ] **Approvals & Health**:
    - `manage_approval` tool to integrate with the approvals API.
    - `check_health` tool to expose circuit breaker and gateway status.

## Phase 5: UI & UX (Prompts & SSE)
- [ ] **SSE Transport**:
    - Add support for HTTP/SSE transport for web-based MCP hosts.
- [ ] **Prompts Library**:
    - Hardcode initial prompts for incident analysis and alert tuning.
- [ ] **Documentation**:
    - Add `README.md` to `crates/mcp-server`.
    - Provide example configuration for Claude Desktop.

## Phase 6: Testing & Validation
- [ ] **Unit Tests**: Test tool parameter mapping and error handling.
- [ ] **Integration Tests**: Run the MCP server in a test harness against a mock gateway.
- [ ] **Manual Validation**: Test with Claude Desktop or MCP Inspector.
