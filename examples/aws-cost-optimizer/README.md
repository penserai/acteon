# AWS Cost Optimizer

A practical example using **Recurring Actions** with the **AWS Auto Scaling** and **EC2** providers to automatically scale down staging Auto Scaling Groups during off-hours and restore them each morning. Includes safety guardrails that block destructive EC2 operations unless capacity headroom is attested. This reduces overnight compute costs by ~87% for non-production environments.

## Features Exercised

| # | Feature | How |
|---|---------|-----|
| 1 | **AWS Auto Scaling** | `set_desired_capacity` to scale ASGs up and down |
| 2 | **AWS EC2** | Instance-level operations (terminate, reboot) with safety guardrails |
| 3 | **Recurring Actions** | 6 cron-scheduled jobs (3 scale-down, 3 scale-up) |
| 4 | **Timezone-aware cron** | Schedules use `America/New_York` timezone |
| 5 | **Quotas** | 100 scaling actions/day limit |
| 6 | **Circuit breakers** | ASG provider trips after 2 failures, falls back to log |
| 7 | **Rules** | Safety guardrails + enrichment for scaling actions |
| 8 | **Audit trail** | Full audit of all scaling operations |

## Prerequisites

- [LocalStack](https://localstack.cloud/) (provides Auto Scaling API)
- `awslocal` CLI (`pip install awscli-local`)
- `jq` (for script output formatting)

## Quick Start

```bash
# 1. Start LocalStack
docker run --rm -d --name localstack -p 4566:4566 localstack/localstack

# 2. Create Auto Scaling Groups
bash examples/aws-cost-optimizer/scripts/setup.sh

# 3. Start Acteon
cargo run -p acteon-server -- -c examples/aws-cost-optimizer/acteon.toml

# 4. Create recurring scaling schedules and quota
bash examples/aws-cost-optimizer/scripts/setup-api.sh

# 5. (Optional) Send manual scaling actions to test the pipeline
bash examples/aws-cost-optimizer/scripts/send-scaling.sh

# 6. View report
bash examples/aws-cost-optimizer/scripts/show-report.sh

# 7. Cleanup API resources
bash examples/aws-cost-optimizer/scripts/teardown.sh

# 8. Stop LocalStack
docker stop localstack
```

## Guardrails

The example includes **safety rules** (`rules/safety.yaml`) that block destructive EC2 and ASG operations unless the caller explicitly attests that capacity headroom has been verified. These rules run at priority 1-2 so they fire before routing.

### What the rules protect against

| Rule | Action Type | Blocks when |
|------|------------|-------------|
| `require-capacity-check-terminate` | `terminate_instances` | `capacity_verified` is not `true` |
| `require-capacity-check-reboot` | `reboot_instances` | `capacity_verified` is not `true` |
| `block-zero-capacity-critical` | `set_desired_capacity` | `desired_capacity < 1` on non-worker ASGs |
| `minimum-capacity-api` | `set_desired_capacity` | `desired_capacity < 1` on `staging-api` |

### The `capacity_verified` attestation pattern

Acteon rules can't query AWS in real time, so the pattern is:

1. **Client** calls `describe_auto_scaling_groups` to check current capacity
2. **Client** verifies `desired_capacity > min_size + N` (enough headroom)
3. **Client** includes `"capacity_verified": true` and `"available_capacity": N` in the payload
4. **Acteon** rules allow the destructive operation because the attestation is present

This keeps the safety check at the caller while Acteon enforces that the check was performed. The `available_capacity` field is recorded in the audit trail for post-incident review.

### Testing the guardrails

`send-scaling.sh` includes test cases at the end that exercise each safety rule. Expected outcomes are noted in the dispatch descriptions (DENIED vs ALLOWED).

## File Structure

```
aws-cost-optimizer/
├── acteon.toml              # Server config (EC2 + ASG providers, background recurring, quota)
├── rules/
│   ├── safety.yaml          # Guardrail rules (deny destructive ops without attestation)
│   └── routing.yaml         # Route scaling actions, enrich metadata, catch-all
├── scripts/
│   ├── setup.sh             # Create LocalStack ASGs (staging-web, staging-api, staging-workers)
│   ├── setup-api.sh         # Create 6 recurring actions + quota via API
│   ├── send-scaling.sh      # Fire manual scaling actions + guardrail tests
│   ├── show-report.sh       # Query audit/health/recurring/quotas
│   └── teardown.sh          # Clean up API-created resources
└── README.md
```

## Scaling Schedule

```
                Mon-Fri
     7am EST              7pm EST
        │                    │
        ▼                    ▼
  ┌──────────┐         ┌──────────┐
  │ Scale Up │         │Scale Down│
  └──────────┘         └──────────┘

  staging-web:     4 instances  ──►  1 instance
  staging-api:     6 instances  ──►  1 instance
  staging-workers: 5 instances  ──►  0 instances
  ─────────────────────────────────────────────
  Total:          15 instances  ──►  2 instances
                                    (87% reduction)
```

The recurring actions fire Monday through Friday:
- **Scale-down** at 7pm EST (`0 19 * * 1-5`): reduce each ASG to minimum capacity
- **Scale-up** at 7am EST (`0 7 * * 1-5`): restore daytime capacity

Weekends keep off-hours capacity since the cron expression excludes Saturday/Sunday.

## How It Works

1. **`setup.sh`** creates 3 Auto Scaling Groups in LocalStack with daytime capacity
2. **`setup-api.sh`** creates 6 recurring actions via the Acteon REST API, each with:
   - A cron expression (e.g., `0 19 * * 1-5`)
   - A timezone (`America/New_York`)
   - An action template targeting the `cost-asg` provider with `set_desired_capacity`
3. **Acteon's background processor** evaluates cron schedules every 30 seconds
4. When a schedule fires, it dispatches the action through the full pipeline (rules, quotas, circuit breakers)
5. The `cost-asg` provider calls the AWS Auto Scaling `SetDesiredCapacity` API
6. All operations are recorded in the audit trail

## Notes

- **LocalStack** provides the Auto Scaling API locally. The provider endpoint points to `http://localhost:4566`.
- **In-memory state** is used for simplicity. For production, use Redis or DynamoDB.
- **Recurring actions** use the CAS (compare-and-swap) claim pattern to prevent duplicate execution in multi-instance deployments.
- **`honor_cooldown: false`** is set on scaling actions to ensure immediate execution regardless of ASG cooldown periods.
- **Weekend handling**: The `1-5` day-of-week range means no scaling happens on weekends, keeping capacity at whatever state it was in Friday evening.
- **`send-scaling.sh`** sends the same actions manually, useful for testing without waiting for cron triggers.
