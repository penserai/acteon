# Prometheus alerting rules

Acteon can generate a Prometheus rule file from the running, sanitized server
configuration:

```bash
acteon metrics export-alerts --output acteon-alerts.yml
```

The same document is available from the public endpoint
`GET /v1/metrics/alerts/prometheus.yaml`, which is suitable for a deployment
job or a mounted Prometheus rules directory. The endpoint contains no secrets
or tenant labels.

Rules are emitted for the features enabled in the configuration:

- missing metrics and recent dispatch failures;
- low provider success rate (below 95%) and high p99 latency (above 5 seconds);
- circuit breaker trips and quota exceedances;
- compliance retention errors; and
- audit or dead-letter retention windows shorter than one day.

The existing `/metrics/prometheus` endpoint now exports the audit and
dead-letter retention TTLs used by the retention warnings. A zero value means
that retention is indefinite. Thresholds are intentionally conservative
defaults; review generated rules against your service-level objectives before
deploying them.
