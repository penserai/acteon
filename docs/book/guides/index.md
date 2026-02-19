# Guides

In-depth guides that show how to combine Acteon's features to solve real-world problems.

<div class="grid" markdown>

<div class="card" markdown>
### [AI Agent Swarm Coordination](agent-swarm-coordination.md)
Use Acteon as a safety and orchestration layer for multi-agent AI systems. Covers identity, permissions, prompt injection defense, rate limiting, approval workflows, and observability.
</div>

<div class="card" markdown>
### [AWS Event-Driven Pipeline](aws-event-pipeline.md)
Build an IoT telemetry pipeline with Acteon routing sensor data to AWS services (SNS, Lambda, EventBridge, SQS, S3). Covers chain orchestration, circuit breaker fallbacks, event grouping, and a full LocalStack development setup.
</div>

<div class="card" markdown>
### [Incident Response Pipeline](incident-response-pipeline.md)
Route monitoring alerts through Acteon for triage, dedup, throttle, and multi-step escalation. Covers chain orchestration with war-room sub-chains, event lifecycle management, circuit breaker fallbacks, and recurring health checks.
</div>

<div class="card" markdown>
### [E-Commerce Order Pipeline](ecommerce-order-pipeline.md)
Process e-commerce orders through Acteon with fraud screening, business-hours scheduling, approval gates, and multi-step fulfillment chains. Covers time-based conditions, SSE streaming, order lifecycle tracking, and payment field redaction.
</div>

<div class="card" markdown>
### [Healthcare Notification Pipeline](healthcare-notification-pipeline.md)
Build a HIPAA-compliant notification gateway that detects and blocks PHI over insecure channels, reroutes sensitive data to a patient portal, and maintains a tamper-evident hash-chained audit trail. Covers compliance mode, approval workflows, and PHI redaction.
</div>

</div>
