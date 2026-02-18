# AWS Cost Optimizer

A practical example using **Recurring Actions** with the **AWS Auto Scaling** provider to automatically scale down staging Auto Scaling Groups during off-hours and restore them each morning. This reduces overnight compute costs by ~87% for non-production environments.

## Features Exercised

| # | Feature | How |
|---|---------|-----|
| 1 | **AWS Auto Scaling** | `set_desired_capacity` to scale ASGs up and down |
| 2 | **Recurring Actions** | 6 cron-scheduled jobs (3 scale-down, 3 scale-up) |
| 3 | **Timezone-aware cron** | Schedules use `America/New_York` timezone |
| 4 | **Quotas** | 100 scaling actions/day limit |
| 5 | **Circuit breakers** | ASG provider trips after 2 failures, falls back to log |
| 6 | **Rules** | Enrich scaling actions with cost-optimization metadata |
| 7 | **Audit trail** | Full audit of all scaling operations |

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

## File Structure

```
aws-cost-optimizer/
├── acteon.toml              # Server config (ASG provider, background recurring, quota)
├── rules/
│   └── routing.yaml         # Route scaling actions, enrich metadata, catch-all
├── scripts/
│   ├── setup.sh             # Create LocalStack ASGs (staging-web, staging-api, staging-workers)
│   ├── setup-api.sh         # Create 6 recurring actions + quota via API
│   ├── send-scaling.sh      # Fire manual scaling actions (no waiting for cron)
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
