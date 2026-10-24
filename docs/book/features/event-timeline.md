# Event timeline and CDC ledger

The timeline stores the latest version of a tenant's events. A separate CDC ledger
records the changes that produced those versions. Events belong to a namespace,
tenant, source, and event ID. `eventGroup` bundles events for a unit of work, such
as a cluster deployment, including events produced by different sources.

An update replaces the searchable event. A delete removes it from queries while
retaining its CDC history. Expired and deleted identities cannot be recreated:
use a new event ID for a new lifecycle. This subsystem is independent from action
audit, state-machine events, and notification groups.

## Configuration

See [`examples/timeline.toml`](https://github.com/penserai/acteon/blob/main/examples/timeline.toml)
for ClickHouse action audit alongside an Elasticsearch timeline. Timeline supports
`elasticsearch`, `clickhouse`, and development-only `memory`. `backend = "audit"`
explicitly inherits audit's engine and URL; it still uses its own collection.
Unsupported inherited engines fail startup. It does not inherit audit redaction,
encryption, compliance holds, or action retention settings.

The subsystem is disabled by default. Enabled deployments require a durable state
backend for revision fences and pending work. `allow_memory = true` permits local
development with memory state/projections but does not provide production durability.
Use a distinct ledger directory for each server instance. An exclusive directory
lock prevents two processes from concurrently owning one local ledger.

| Retained data | Default | Tenant policy override |
|---|---|---|
| Current timeline event | Seven days from first ingestion | `timeline_ttl_seconds` |
| Local CDC ledger | 24 hours from receipt | `cdc_ledger_ttl_seconds` |
| Optional cold CDC archive | 30 days from receipt | `cdc_archive_ttl_seconds` |

Retention accepts one second through ten years. New policies affect subsequently
admitted identities/changes; updates and replay never renew event expiry. Queries
exclude expired events immediately. Physical cleanup runs periodically. Pending
ledger records remain pinned until processing completes, even after their normal
retention deadline. Exhausted disk capacity backpressures ingestion.

Compact identity/version fences persist after payload expiry to reject old
messages and prevent resurrection. They are not part of the seven-day payload TTL.

## Ingest an event

`POST /v1/timeline/changes`:

```json
{
  "scope": { "namespace": "deployments", "tenant": "acme" },
  "change": {
    "schemaVersion": 1,
    "changeId": "deployment-42-change-1",
    "source": "deployment-controller",
    "eventId": "deployment-42",
    "revision": 1,
    "operation": "create",
    "after": {
      "eventGroup": "cluster-eu-42",
      "occurredAt": "2026-10-24T12:00:00Z",
      "eventType": "cluster.deployment",
      "message": "Deployment started",
      "attributes": {
        "deploymentId": "550e8400-e29b-41d4-a716-446655440000",
        "region": "eu-west-1",
        "attempt": 1
      }
    }
  }
}
```

Use `update` with a complete `after` image and a higher revision to change an
event. `snapshot` also supplies a complete image. `delete` omits `after`. `before`
is optional. Infrastructure timestamps, expiry, and applied revision are assigned
by Acteon and cannot be supplied inside the event body.

One authoritative source owns each event. Canonical revisions are positive signed
64-bit integers. Repeated identical changes return `duplicate`; conflicting
content at the same revision returns HTTP 409. Older changes are retained as
`stale`. Later changes to deleted/expired events return `terminal` and do not
restore the projection. Unknown envelope/body fields are rejected; place custom
fields in `attributes`.

A successful response means the local ledger is fsynced, the projection write is
accepted by the configured backend, and any configured archive manifest has been
committed. Search visibility follows backend refresh. A failed response may still
have persisted some work: retry with the same change ID and revision. A local-only
ledger cannot preserve history if its disk is lost. Backend replication and write
acknowledgement settings remain deployment responsibilities.

`POST /v1/timeline/changes/batch` accepts `{ "scope": ..., "changes": [...] }`
with 1–500 changes, returning an ordered `results` array of receipts/errors. It is
not an atomic multi-event transaction. The server's HTTP body-size limit also applies.
Individual envelopes are limited to 256 KiB; attributes allow depth 16 and 1,000
scalar leaves. Attribute paths are limited to 4,096 bytes, and individual searchable
words to 8,192 bytes. Occurrence timestamps use the shared nanosecond range
1970–2262.

## Search

`POST /v1/timeline/query`:

```json
{
  "scope": { "namespace": "deployments", "tenant": "acme" },
  "query": {
    "eventGroup": "cluster-eu-42",
    "equals": [
      { "path": "/deploymentId", "value": "550e8400-e29b-41d4-a716-446655440000" }
    ],
    "text": { "text": "deployment started", "mode": "all" },
    "limit": 50
  }
}
```

Equality is typed, case sensitive, and matches the complete scalar value. Strings
are not coerced into numbers. A UUID is compared literally. Attribute paths use
JSON Pointer escaping (`~0` for `~`, `~1` for `/`). Scalar arrays support membership
at the array's path; correlated array-object predicates and whole-object equality
are not supported.

Full-text search tokenizes Unicode words and lowercases them identically on both
backends. It searches message and attribute string values. Set `text.path` to a
JSON Pointer to restrict it to an attribute; `mode` is `all` or `any`. Stemming,
fuzzy matching, phrase search, and backend query languages are not exposed.

Additional filters: `eventType`, `from`, `to` (inclusive occurrence-time bounds),
and `exists` (attribute paths). Filters combine with AND. Group queries default to
ascending occurrence time; general queries default descending. `ascending`
overrides that choice. Ties use source and event ID.

Pass `nextCursor` back as `query.cursor` without changing the query or scope.
Pages default to 50 records and are capped at 1,000. Pagination is a live view:
updates between requests can move records across page boundaries.

### CEL filters

Set `query.filter` to a CEL predicate. It is compiled into the same validated
query tree as the structured filters, then into Elasticsearch Query DSL or
ClickHouse SQL. Filtering runs in the backend before cursor pagination. It does
not fetch rows for application-side evaluation. Existing filters remain supported;
when both are supplied, they are combined with AND.

```json
{
  "scope": { "namespace": "deployments", "tenant": "acme" },
  "query": {
    "eventGroup": "cluster-eu-42",
    "filter": "attributes.region in [\"eu-west-1\", \"us-west-2\"] && (attributes.attempt == 1 || attributes.attempt == 2) && textMatch(\"deployment started\")",
    "limit": 50
  }
}
```

The supported subset is deliberately restricted to indexed operations:

| Operation | Example |
|---|---|
| AND, OR, NOT, parentheses | `attributes.ready == true && (eventType == "ready" || eventType == "updated")` |
| Exact scalar equality / inequality | `attributes.deploymentId == "550e8400-e29b-41d4-a716-446655440000"` |
| Nested attributes / literal keys | `attributes.cluster.id == "cluster-42"`; `attributes["cluster.id"] == "literal-key"` |
| Attribute presence | `has(attributes.region)`; `!has(attributes.error)` |
| Field membership in a literal list | `attributes.region in ["eu-west-1", "us-west-2"]` |
| Scalar membership in an attribute array | `"production" in attributes.tags` |
| Event-time bounds | `occurredAt >= timestamp("2026-10-24T00:00:00Z")` |
| All search words | `textMatch("deployment started")`; `textMatch(attributes.description, "deployment started")` |
| Any search word | `textAny(attributes.description, "failed timeout")` |

Available infrastructure fields are `eventGroup`, `eventType`, `source`, `eventId`
(string equality and list membership), and `occurredAt` (timestamp equality,
inequality, and ranges). Namespace, tenant, retention, and deletion constraints
are always imposed separately; expressions cannot override them.

`textMatch` and `textAny` are Acteon extensions, not standard CEL functions. They
use the same Unicode word tokenization as structured `text`. Without an attribute
argument, they search the message and all attribute strings. They return false
when there are no matching searchable words at the requested path.

Exact strings are case sensitive and never tokenized. Numbers, strings, and
booleans are not coerced; integer and decimal representations are distinct in
this subset. A field reference must be on the left of `==` / `!=`; the right
side must be a literal (or `timestamp(...)` for `occurredAt`). Strings use double
quotes; supported escapes are `\\`, `\"`, `\n`, and `\t`. Use literal Unicode
characters. Numeric literals are signed integers or decimal fractions.

Presence and collections have explicit semantics:

- Null is a present value: `has(attributes.x)` and `attributes.x == null` both
  match `{ "x": null }`. Empty objects and arrays are also present.
- A comparison against a missing field is unresolved and does not select that
  event, including `!=` and negated comparisons. Boolean `true || ...` and
  `false && ...` still resolve normally. Use `!has(attributes.x)` to find absence.
- Unlike legacy `equals`, CEL scalar equality does **not** match array members.
  Use `"blue" in attributes.tags` for arrays. An array membership expression
  against a missing field or a non-array is unresolved.
- Object traversal does not flatten arrays. Whole-object/list equality, array
  subscripts, and correlated predicates across array objects are unsupported.

This is a CEL **subset**, not a general CEL evaluator. Attribute ranges,
arithmetic, regex, comprehensions, ternaries, arbitrary functions, single-quoted
strings, and field-to-field comparisons return HTTP 400. Expressions are limited
to 4,096 bytes, 256 parsed nodes, and depth 32; the parser may impose a tighter
limit on heavily parenthesized expressions. Keep the exact same `filter`, scope,
and other query options when using `nextCursor`. Default ordering still depends
on the structured `eventGroup` option; set `ascending` explicitly if the group
condition is written only inside CEL.

Open **Event Timeline** in the admin navigation (`/timeline`) to query a namespace
and tenant. The page provides a multiline CEL editor with examples and
Ctrl/Command+Enter submission. Event group and event type are available directly;
**Additional filters** includes full-text scope and word matching, local occurrence
time bounds (converted to UTC), and repeatable typed attribute conditions. Choose
String, Number, Boolean, Null, or Exists for each condition. All conditions combine
with AND; use CEL for OR and NOT. UUIDs should use String. Use CEL for integers
above JavaScript’s safe range or integral decimals such as `2.0`; the Number form
rejects these rather than changing their representation during JSON serialization.

Choose the result order and page size before submitting. Previous/next navigation
preserves the submitted query even if the form is edited; an explicit notice marks
unapplied changes. Previous pages show cached results, while new pages read live
data. Search again to refresh. Changing namespace or tenant clears results and
cancels outstanding requests. Selecting an event opens its identity, timestamps,
and expandable event JSON. Ledger status is loaded separately for the selected scope.

All SDK query helpers accept `filter` in the query object; Rust exposes it as
`TimelineQuery.filter`:

```python
client.query_timeline("deployments", "acme", {
    "filter": 'attributes.region == "eu-west-1" && !has(attributes.error)',
    "limit": 50,
})
```

CEL adds namespaced scalar/presence entries to the existing fixed keyword
indexes. Projections created by an earlier preview without these entries must be
rebuilt before using CEL attribute filters; changing the mapping alone is not
sufficient. No dynamic per-attribute backend mappings are introduced.

`GET /v1/timeline/events/{source}/{eventId}?namespace=...&tenant=...` returns the
current event or 404. Every request checks both role permission and tenant/namespace
grants. Read permission is available to existing roles; ingest is available to
admin/operator/executor; ledger administration is available to admin/operator.

## Kafka and Debezium

With the server's `bus` feature, configured `timeline.sources` use acknowledged
subscriptions and the same ingestion boundary as REST. Offsets are acknowledged
only after processing. Transient failures retry with backoff. Invalid messages
halt the source without advancing the offset; repair the source and restart it.
Set a source's `poison_policy = "quarantine"` to retain decoded JSON that fails
schema/semantic validation or revision checks, then advance its offset only after
ledger persistence and any configured archival. Quarantined records use the
reserved ledger operation `rejected` and disposition `quarantined`; their source
binding and rejection category are in transport metadata. They never enter timeline
search. Producers cannot submit the reserved operation or `_acteon_` sources.
Transport decoding failures, lane/source binding faults, and records exceeding the
retention-size limits still halt ingestion. Quarantine shares the configured ledger
capacity and retention, so it cannot grow past the disk budget.

Canonical JSON envelopes use their supplied revision. The PostgreSQL Debezium
adapter accepts JSON with or without the schema wrapper and supports initial
snapshots, create, update, delete, and null Kafka tombstones. Source keys identify
events; row fields are mapped through configured JSON Pointers. `occurred_at`
must map to RFC3339 text. The original envelope and transport coordinates are
preserved in the ledger.

The adapter identifies sources as `source@generation` and derives revision from
Kafka offset + 1. It binds each event to its original topic/partition so offsets
from different partitions cannot be compared. Reset/repartition operations require
a new source generation. PostgreSQL source positions remain available in raw CDC
history. A null Kafka tombstone has no additional projection effect. Incremental
snapshots, truncate events, Avro/Protobuf/schema-registry decoding, and arbitrary
source transformations are not supported.

## CDC archival and replay

The optional `timeline-archive` feature writes Zstandard-compressed Parquet to
S3-compatible storage. Credentials come from the AWS environment/credential chain.
Objects are partitioned by hashed namespace and tenant, then receipt date/hour.
Those hashes are path components, not an authorization mechanism: grant external
readers storage access only to their intended prefixes.

Each partition's `manifest.json` lists committed objects, SHA-256 checksums, row
counts, and expiry bounds. Publication uses conditional manifest updates. Readers
must read the manifest and its listed objects rather than globbing every Parquet
file. Retries can produce repeated ledger IDs; deduplicate on `entry_json.id`.
Compaction replaces manifest references, preserving source envelopes. Unreferenced
objects have a 24-hour garbage-collection grace period for external readers.
Filter rows by `archive_until` at query time; object expiry is the latest row expiry
in that file.

Parquet columns include namespace, tenant, source, event identity, revision,
operation, receipt/expiry times, group, disposition, transport coordinates,
before/after, and the complete original envelope. JSON columns preserve dynamically
growing attributes without requiring a schema migration.

For example, after resolving a manifest's object list into `committed_files`, use
DuckDB with its configured S3 credentials:

```sql
SELECT event_id, revision, operation,
       json_extract_string(after_json, '$.attributes.deploymentId') AS deployment_id
FROM read_parquet(committed_files)
WHERE namespace = 'deployments' AND tenant = 'acme'
  AND CAST(archive_until AS TIMESTAMPTZ) > current_timestamp
  AND event_group = 'cluster-eu-42';
```

ClickHouse's `s3()` table function and Athena external Parquet tables can read the
same committed object set. Exclude obsolete/uncommitted objects and apply the
same expiry and ledger-ID deduplication rules.

`GET /v1/timeline/ledger/status` reports scoped local records, bytes, pending work,
and oldest receipt. `POST /v1/timeline/replays` accepts a scope and returns a job;
`GET /v1/timeline/replays/{id}` reports its status. Replay reapplies retained local
records against existing durable version fences. It is a projection repair tool,
not reconstruction of a lost coordination store. Cold archive queries are external;
cold-archive replay/import is not exposed by the first HTTP implementation.

## Validation and capacity

Run `cargo run -p acteon-simulation --example timeline_deployment` for a standalone
create/update/delete example. Conformance tests cover state recovery, version
fencing, search semantics, and expiry. Real-backend tests use:

```bash
cargo test -p acteon-timeline --test backends -- --ignored
cargo test -p acteon-timeline --all-features --lib --tests
```

Targets are 1,000 changes/second, 10 million live events, p95 group queries below
200 ms, search below 500 ms, and visibility within two seconds. These are capacity
qualification targets, not measured guarantees. The initial local ledger uses
immutable per-record segments; storage throughput, inode usage, and maintenance
cost must be measured for the intended retention volume. ClickHouse's latest-row
resolution and attribute filtering also require workload-specific qualification.
Prometheus exports aggregate timeline ingestion/query counters and accumulated
operation durations without tenant IDs or payloads in labels.
