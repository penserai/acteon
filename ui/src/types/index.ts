// ---- Metrics ----
export interface MetricsResponse {
  dispatched: number
  executed: number
  deduplicated: number
  suppressed: number
  rerouted: number
  throttled: number
  failed: number
  pending_approval?: number
  chains_started: number
  chains_completed: number
  chains_failed: number
  chains_cancelled: number
  circuit_open?: number
  scheduled?: number
  embedding?: EmbeddingMetrics
}

export interface EmbeddingMetrics {
  similarity_requests: number
  similarity_errors: number
  topic_cache_hits: number
  topic_cache_misses: number
  text_cache_hits: number
  text_cache_misses: number
}

export interface HealthResponse {
  status: string
  metrics: MetricsResponse
}

// ---- Dispatch ----
export interface DispatchRequest {
  namespace: string
  tenant: string
  provider: string
  action_type: string
  payload: Record<string, unknown>
  metadata?: Record<string, string>
  dedup_key?: string
  fingerprint?: string
  status?: string
  starts_at?: string
  ends_at?: string
}

export interface DispatchResponse {
  action_id: string
  outcome: string
  details: Record<string, unknown>
}

// ---- Rules ----
export interface RuleSummary {
  name: string
  priority: number
  description?: string
  enabled: boolean
  action_type: string
  action_details: Record<string, unknown>
  source: string
  version?: number
}

// ---- Audit ----
export interface AuditRecord {
  id: string
  action_id: string
  chain_id?: string
  namespace: string
  tenant: string
  provider: string
  action_type: string
  verdict: string
  matched_rule?: string
  outcome: string
  action_payload?: Record<string, unknown>
  verdict_details: Record<string, unknown>
  outcome_details: Record<string, unknown>
  metadata: Record<string, string>
  dispatched_at: string
  completed_at: string
  duration_ms: number
  expires_at?: string
  caller_id: string
  auth_method: string
}

export interface AuditPage {
  records: AuditRecord[]
  total: number
  limit: number
  offset: number
}

export interface AuditQuery {
  namespace?: string
  tenant?: string
  provider?: string
  action_type?: string
  outcome?: string
  verdict?: string
  matched_rule?: string
  caller_id?: string
  chain_id?: string
  from?: string
  to?: string
  limit?: number
  offset?: number
}

export interface ReplayResult {
  original_action_id: string
  new_action_id: string
  success: boolean
  error?: string
}

export interface ReplaySummary {
  replayed: number
  failed: number
  skipped: number
  results: ReplayResult[]
}

// ---- Chains ----
export interface ChainSummary {
  chain_id: string
  chain_name: string
  status: string
  current_step: number
  total_steps: number
  started_at: string
  updated_at: string
}

export interface ChainStepStatus {
  name: string
  provider: string
  status: string
  response_body?: Record<string, unknown>
  error?: string
  completed_at?: string
}

export interface ChainDetailResponse {
  chain_id: string
  chain_name: string
  status: string
  current_step: number
  total_steps: number
  steps: ChainStepStatus[]
  started_at: string
  updated_at: string
  expires_at?: string
  cancel_reason?: string
  cancelled_by?: string
  execution_path: string[]
}

export interface BranchCondition {
  field: string
  operator: 'Eq' | 'Neq' | 'Contains' | 'Exists'
  value?: unknown
  target: string
}

export interface ChainStepConfig {
  name: string
  provider: string
  action_type: string
  payload_template: Record<string, unknown>
  on_failure?: string
  delay_seconds?: number
  branches: BranchCondition[]
  default_next?: string
}

// ---- Approvals ----
export interface ApprovalStatus {
  approval_id: string
  action_id: string
  status: string
  rule: string
  message?: string
  created_at: string
  expires_at: string
  decided_by?: string
  decided_at?: string
  namespace?: string
  tenant?: string
  action_type?: string
  provider?: string
  payload?: Record<string, unknown>
}

// ---- Circuit Breakers ----
export interface CircuitBreakerStatus {
  provider: string
  state: 'closed' | 'open' | 'half_open'
  failure_threshold: number
  success_threshold: number
  recovery_timeout_seconds: number
  fallback_provider?: string
}

// ---- DLQ ----
export interface DlqStats {
  enabled: boolean
  count: number
}

// ---- Events ----
export interface EventState {
  state: string
  fingerprint: string
  updated_at: string
  transitioned_by: string
  state_machine?: string
}

// ---- Groups ----
export interface GroupedEvent {
  action_id: string
  fingerprint?: string
  status?: string
  payload: Record<string, unknown>
  received_at: string
}

export interface EventGroup {
  group_id: string
  group_key: string
  labels: Record<string, string>
  events: GroupedEvent[]
  notify_at: string
  state: 'Pending' | 'Notified' | 'Resolved'
  created_at: string
  updated_at: string
}

// ---- Stream ----
export interface StreamEvent {
  id: string
  timestamp: string
  event_type: string
  namespace: string
  tenant: string
  action_type: string
  action_id: string
}

// ---- Auth ----
export interface LoginRequest {
  username: string
  password: string
}

export interface LoginResponse {
  token: string
}

// ---- Similarity ----
export interface SimilarityRequest {
  text: string
  topic: string
}

export interface SimilarityResponse {
  similarity: number
  topic: string
}

// ---- Recurring Actions ----

/** Summary returned in list responses. */
export interface RecurringActionSummary {
  id: string
  namespace: string
  tenant: string
  cron_expr: string
  timezone: string
  enabled: boolean
  provider: string
  action_type: string
  next_execution_at: string | null
  execution_count: number
  description: string | null
  created_at: string
}

/** Full detail returned by GET /v1/recurring/:id. */
export interface RecurringAction extends RecurringActionSummary {
  payload: Record<string, unknown>
  metadata: Record<string, string>
  dedup_key: string | null
  last_executed_at: string | null
  ends_at: string | null
  max_executions: number | null
  labels: Record<string, string>
  updated_at: string
}

export interface RecurringActionListResponse {
  recurring_actions: RecurringActionSummary[]
  count: number
}

export interface CreateRecurringActionRequest {
  namespace: string
  tenant: string
  cron_expression: string
  timezone: string
  provider: string
  action_type: string
  payload: Record<string, unknown>
  metadata?: Record<string, string>
  dedup_key?: string | null
  description?: string | null
  ends_at?: string | null
  max_executions?: number | null
  enabled?: boolean
}

export interface CreateRecurringActionResponse {
  id: string
  name: string | null
  next_execution_at: string | null
  status: string
}

export interface UpdateRecurringActionRequest {
  namespace: string
  tenant: string
  name?: string | null
  cron_expression?: string
  timezone?: string
  enabled?: boolean
  provider?: string
  action_type?: string
  payload?: Record<string, unknown>
  metadata?: Record<string, string>
  dedup_key?: string | null
  description?: string | null
  max_executions?: number | null
  ends_at?: string | null
}

export interface PauseResumeResponse {
  id: string
  enabled: boolean
  next_execution_at: string | null
}

// ---- Config ----
export interface ConfigResponse {
  server: {
    host: string
    port: number
    shutdown_timeout_seconds: number
    external_url: string | null
    max_sse_connections_per_tenant: number | null
  }
  state: {
    backend: string
    has_url: boolean
    prefix: string | null
    region: string | null
    table_name: string | null
  }
  executor: {
    max_retries: number | null
    timeout_seconds: number | null
    max_concurrent: number | null
    dlq_enabled: boolean
  }
  rules: {
    directory: string | null
    default_timezone: string | null
  }
  audit: {
    enabled: boolean
    backend: string
    has_url: boolean
    prefix: string
    ttl_seconds: number | null
    cleanup_interval_seconds: number
    store_payload: boolean
    redact: { enabled: boolean; field_count: number; placeholder: string }
  }
  auth: {
    enabled: boolean
    config_path: string | null
    watch: boolean | null
  }
  rate_limit: {
    enabled: boolean
    config_path: string | null
    on_error: string
  }
  llm_guardrail: {
    enabled: boolean
    endpoint: string
    model: string
    has_api_key: boolean
    policy: string
    policy_keys: string[]
    fail_open: boolean
    timeout_seconds: number | null
    temperature: number | null
    max_tokens: number | null
  }
  embedding: {
    enabled: boolean
    endpoint: string
    model: string
    has_api_key: boolean
    timeout_seconds: number
    fail_open: boolean
    topic_cache_capacity: number
    topic_cache_ttl_seconds: number
    text_cache_capacity: number
    text_cache_ttl_seconds: number
  }
  circuit_breaker: {
    enabled: boolean
    failure_threshold: number
    success_threshold: number
    recovery_timeout_seconds: number
    provider_overrides: string[]
  }
  background: {
    enabled: boolean
    enable_group_flush: boolean
    enable_timeout_processing: boolean
    enable_approval_retry: boolean
    enable_scheduled_actions: boolean
    group_flush_interval_seconds: number
    timeout_check_interval_seconds: number
    cleanup_interval_seconds: number
    scheduled_check_interval_seconds: number
  }
  telemetry: {
    enabled: boolean
    endpoint: string
    service_name: string
    sample_ratio: number
    protocol: string
    timeout_seconds: number
    resource_attribute_keys: string[]
  }
  chains: {
    max_concurrent_advances: number
    completed_chain_ttl_seconds: number
    definitions: Array<{ name: string; steps_count: number; timeout_seconds: number | null }>
  }
  providers: Array<{ name: string; provider_type: string; url: string | null; header_count: number }>
}
