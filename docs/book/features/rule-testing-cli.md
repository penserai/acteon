# Rule Testing CLI

The rule testing CLI lets you run automated test suites against your routing
rules. Define expected verdicts and matched-rule names in a YAML fixture file,
then execute the suite with a single command. This is ideal for CI/CD
validation before deploying rule changes.

## Quick Start

Create a fixture file (e.g. `tests/rules.yaml`):

```yaml
tests:
  - name: "spam is suppressed"
    action:
      namespace: notifications
      tenant: acme
      provider: email
      action_type: spam
      payload: {}
    expect:
      verdict: suppress
      matched_rule: block-spam

  - name: "normal email is allowed"
    action:
      namespace: notifications
      tenant: acme
      provider: email
      action_type: send_email
      payload: { to: "user@example.com" }
    expect:
      verdict: allow
```

Run the tests (requires a running Acteon server with rules loaded):

```bash
acteon rules test tests/rules.yaml
```

## Fixture Format

Each fixture file is a YAML document with a top-level `tests` array. Every
entry contains:

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Human-readable test name |
| `action.namespace` | Yes | Namespace for the test action |
| `action.tenant` | Yes | Tenant identifier |
| `action.provider` | Yes | Target provider name |
| `action.action_type` | Yes | Action type discriminator |
| `action.payload` | No | JSON payload (defaults to `{}`) |
| `action.metadata` | No | Metadata labels (key-value map) |
| `expect.verdict` | Yes | Expected verdict (`allow`, `suppress`, `deny`, etc.) |
| `expect.matched_rule` | No | Expected matched rule name |

## CLI Usage

```
acteon rules test <FIXTURES> [OPTIONS]

Arguments:
  <FIXTURES>  Path to YAML fixtures file

Options:
  --filter <PATTERN>  Only run tests whose name contains this substring
  --format <FORMAT>   Output format: text (default) or json
```

### Text Output

```
  PASS  normal email is allowed
  FAIL  spam is suppressed (expected verdict 'suppress', got 'allow')

test result: 1 passed, 1 failed (2 total) in 42ms
```

The command exits with code 0 if all tests pass and 1 if any fail.

### JSON Output

```bash
acteon rules test tests/rules.yaml --format json
```

```json
{
  "total": 2,
  "passed": 1,
  "failed": 1,
  "results": [
    {
      "name": "normal email is allowed",
      "passed": true,
      "expected_verdict": "allow",
      "actual_verdict": "allow",
      "duration_us": 1200
    },
    {
      "name": "spam is suppressed",
      "passed": false,
      "expected_verdict": "suppress",
      "actual_verdict": "allow",
      "duration_us": 980
    }
  ],
  "duration_ms": 42
}
```

### Filtering Tests

Run a subset of tests by name substring:

```bash
acteon rules test tests/rules.yaml --filter "spam"
```

## MCP Tool

The `test_rules` MCP tool provides the same functionality for AI assistants:

```json
{
  "fixtures_path": "tests/rules.yaml",
  "filter": "spam"
}
```

Returns the full `TestRunSummary` JSON.

## CI/CD Integration

Add rule testing to your CI pipeline:

```yaml
# GitHub Actions example
- name: Start Acteon server
  run: cargo run -p acteon-server &

- name: Wait for server
  run: sleep 3

- name: Run rule tests
  run: acteon rules test tests/rules.yaml
```

The non-zero exit code on failure integrates naturally with CI systems.
