// React Query hooks for the agentic bus surface (Phases 1–6c).
//
// Each list/get hook polls at 10s — bus state changes (new
// conversations, agent heartbeats, fresh approvals) frequently
// enough that pull-only is fine for V1; a future iteration can
// migrate the long-polling endpoints to SSE on the same
// `EventStream` infra the rest of the UI uses.

import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost, apiPut, apiDelete } from '../client'

// --------------- Topics ---------------

export interface BusTopic {
  name: string
  namespace: string
  tenant: string
  kafka_name: string
  partitions: number
  replication_factor: number
  retention_ms?: number | null
  description?: string | null
  labels?: Record<string, string>
  schema_subject?: string | null
  schema_version?: number | null
  created_at: string
  updated_at: string
}

export interface ListBusTopicsResponse {
  topics: BusTopic[]
  count: number
}

export interface CreateBusTopicReq {
  name: string
  namespace: string
  tenant: string
  partitions?: number
  replication_factor?: number
  retention_ms?: number
  description?: string
  labels?: Record<string, string>
}

export function useBusTopics(filter: { namespace?: string; tenant?: string } = {}) {
  return useQuery({
    queryKey: ['bus', 'topics', filter],
    queryFn: async () => {
      const res = await apiGet<ListBusTopicsResponse>('/v1/bus/topics', filter)
      return res.topics
    },
    refetchInterval: 10_000,
  })
}

export function useCreateBusTopic() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (req: CreateBusTopicReq) => apiPost<BusTopic>('/v1/bus/topics', req),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['bus', 'topics'] }),
  })
}

export function useDeleteBusTopic() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ namespace, tenant, name }: { namespace: string; tenant: string; name: string }) =>
      apiDelete<void>(
        `/v1/bus/topics/${encodeURIComponent(namespace)}/${encodeURIComponent(tenant)}/${encodeURIComponent(name)}`,
      ),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['bus', 'topics'] }),
  })
}

// --------------- Subscriptions ---------------

export interface BusSubscription {
  id: string
  topic: string
  namespace: string
  tenant: string
  starting_offset: string
  ack_mode: string
  dead_letter_topic?: string | null
  ack_timeout_ms: number
  description?: string | null
  labels?: Record<string, string>
  created_at: string
  updated_at: string
}

export interface ListBusSubscriptionsResponse {
  subscriptions: BusSubscription[]
  count: number
}

export interface CreateBusSubscriptionReq {
  id: string
  topic: string
  namespace: string
  tenant: string
  starting_offset?: string
  ack_mode?: string
  dead_letter_topic?: string
  ack_timeout_ms?: number
  description?: string
  labels?: Record<string, string>
}

export interface BusLagPartition {
  partition: number
  committed: number
  high_water_mark: number
  lag: number
}

export interface BusLag {
  subscription_id: string
  topic: string
  partitions: BusLagPartition[]
  total_lag: number
}

export function useBusSubscriptions(
  filter: { namespace?: string; tenant?: string; topic?: string } = {},
) {
  return useQuery({
    queryKey: ['bus', 'subscriptions', filter],
    queryFn: async () => {
      const res = await apiGet<ListBusSubscriptionsResponse>('/v1/bus/subscriptions', filter)
      return res.subscriptions
    },
    refetchInterval: 10_000,
  })
}

export function useBusSubscriptionLag(id: string | undefined) {
  return useQuery({
    queryKey: ['bus', 'subscriptions', id, 'lag'],
    queryFn: () => apiGet<BusLag>(`/v1/bus/subscriptions/${encodeURIComponent(id ?? '')}/lag`),
    // Lag is the metric operators care about most; refresh more
    // aggressively so a stuck consumer pops out within seconds.
    refetchInterval: 5_000,
    enabled: !!id,
  })
}

export function useCreateBusSubscription() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (req: CreateBusSubscriptionReq) =>
      apiPost<BusSubscription>('/v1/bus/subscriptions', req),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['bus', 'subscriptions'] }),
  })
}

export function useDeleteBusSubscription() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) =>
      apiDelete<void>(`/v1/bus/subscriptions/${encodeURIComponent(id)}`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['bus', 'subscriptions'] }),
  })
}

// --------------- Agents ---------------

export interface BusAgent {
  agent_id: string
  namespace: string
  tenant: string
  capabilities: string[]
  inbox_topic: string
  status: string
  last_heartbeat_at?: string | null
  heartbeat_ttl_ms: number
  description?: string | null
  labels?: Record<string, string>
  created_at: string
  updated_at: string
  // Operator lifecycle state — defaults to "active" when a server
  // pre-dating the field returns the agent without it.
  admin_state?: string
  admin_reason?: string | null
  admin_set_by?: string | null
  admin_set_at?: string | null
  admin_expires_at?: string | null
}

export interface ListBusAgentsResponse {
  agents: BusAgent[]
  count: number
}

export function useBusAgents(
  filter: { namespace?: string; tenant?: string; status?: string; admin_state?: string } = {},
) {
  return useQuery({
    queryKey: ['bus', 'agents', filter],
    queryFn: async () => {
      const res = await apiGet<ListBusAgentsResponse>('/v1/bus/agents', filter)
      return res.agents
    },
    // Heartbeats matter — refresh fast so the operator can see an
    // agent flip to `unhealthy` within a few seconds of the TTL
    // elapsing.
    refetchInterval: 5_000,
  })
}

export function useDeleteBusAgent() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ namespace, tenant, agentId }: { namespace: string; tenant: string; agentId: string }) =>
      apiDelete<void>(
        `/v1/bus/agents/${encodeURIComponent(namespace)}/${encodeURIComponent(tenant)}/${encodeURIComponent(agentId)}`,
      ),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['bus', 'agents'] }),
  })
}

// Admin-state mutation. The server validates `expires_at` is only
// supplied alongside `suspended`; this hook just shuttles the body
// over the wire and invalidates the agent list on success.
export interface SetBusAgentAdminState {
  admin_state: 'active' | 'suspended' | 'banned'
  reason?: string
  expires_at?: string  // RFC-3339
}

export function useSetBusAgentAdminState() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({
      namespace,
      tenant,
      agentId,
      body,
    }: {
      namespace: string
      tenant: string
      agentId: string
      body: SetBusAgentAdminState
    }) =>
      apiPut<BusAgent>(
        `/v1/bus/agents/${encodeURIComponent(namespace)}/${encodeURIComponent(tenant)}/${encodeURIComponent(agentId)}/admin-state`,
        body,
      ),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['bus', 'agents'] }),
  })
}

// --------------- Conversations ---------------

export interface BusConversation {
  conversation_id: string
  namespace: string
  tenant: string
  participants: string[]
  state: string
  topic_subject?: string | null
  events_topic?: string | null
  description?: string | null
  labels?: Record<string, string>
  created_at: string
  updated_at: string
}

export interface ListBusConversationsResponse {
  conversations: BusConversation[]
  count: number
}

export interface BusReplayMessage {
  partition: number
  offset: number
  produced_at: string
  sender?: string | null
  payload: unknown
  headers: Record<string, string>
}

export interface BusReplayResponse {
  conversation_id: string
  events_topic: string
  messages: BusReplayMessage[]
  next_cursor?: string | null
  exit_reason: string
}

export function useBusConversations(
  filter: { namespace?: string; tenant?: string; state?: string; participant?: string } = {},
) {
  return useQuery({
    queryKey: ['bus', 'conversations', filter],
    queryFn: async () => {
      const res = await apiGet<ListBusConversationsResponse>('/v1/bus/conversations', filter)
      return res.conversations
    },
    refetchInterval: 10_000,
  })
}

export function useBusConversation(
  namespace: string | undefined,
  tenant: string | undefined,
  id: string | undefined,
) {
  return useQuery({
    queryKey: ['bus', 'conversations', namespace, tenant, id],
    queryFn: () =>
      apiGet<BusConversation>(
        `/v1/bus/conversations/${encodeURIComponent(namespace ?? '')}/${encodeURIComponent(tenant ?? '')}/${encodeURIComponent(id ?? '')}`,
      ),
    enabled: !!(namespace && tenant && id),
    refetchInterval: 10_000,
  })
}

export function useBusConversationMessages(
  namespace: string | undefined,
  tenant: string | undefined,
  id: string | undefined,
  params: { limit?: number; as_agent?: string } = {},
) {
  return useQuery({
    queryKey: ['bus', 'conversations', namespace, tenant, id, 'messages', params],
    queryFn: () =>
      apiGet<BusReplayResponse>(
        `/v1/bus/conversations/${encodeURIComponent(namespace ?? '')}/${encodeURIComponent(tenant ?? '')}/${encodeURIComponent(id ?? '')}/messages`,
        params,
      ),
    enabled: !!(namespace && tenant && id),
    // Threads update as agents post; poll faster than the list view.
    refetchInterval: 5_000,
  })
}

// --------------- Approvals (Phase 6c) ---------------

export type BusApprovalStatus =
  | 'pending'
  | 'approving' // Phase 10: operator approved, produce in flight
  | 'approved'
  | 'rejected'
  | 'expired'

export interface BusApprovalView {
  approval_id: string
  namespace: string
  tenant: string
  conversation_id: string
  correlation_token: string
  envelope_kind: string
  status: BusApprovalStatus
  reason?: string | null
  created_at: string
  expires_at: string
  decided_by?: string | null
  decided_at?: string | null
  decision_note?: string | null
  produced_partition?: number | null
  produced_offset?: number | null
  produced_at?: string | null
  envelope: unknown
}

export interface ListBusApprovalsResponse {
  approvals: BusApprovalView[]
  count: number
}

export function useBusApprovals(
  namespace: string | undefined,
  tenant: string | undefined,
  filter: { status?: BusApprovalStatus; conversation_id?: string } = {},
) {
  return useQuery({
    queryKey: ['bus', 'approvals', namespace, tenant, filter],
    queryFn: () =>
      apiGet<ListBusApprovalsResponse>(
        `/v1/bus/approvals/${encodeURIComponent(namespace ?? '')}/${encodeURIComponent(tenant ?? '')}`,
        filter,
      ),
    enabled: !!(namespace && tenant),
    // Approvals are operator-actioned — short refresh interval so a
    // pending request shows up within seconds of a parking POST.
    refetchInterval: 5_000,
  })
}

export function useBusApproval(
  namespace: string | undefined,
  tenant: string | undefined,
  id: string | undefined,
) {
  return useQuery({
    queryKey: ['bus', 'approvals', namespace, tenant, id],
    queryFn: () =>
      apiGet<BusApprovalView>(
        `/v1/bus/approvals/${encodeURIComponent(namespace ?? '')}/${encodeURIComponent(tenant ?? '')}/${encodeURIComponent(id ?? '')}`,
      ),
    enabled: !!(namespace && tenant && id),
    refetchInterval: 5_000,
  })
}

export interface BusApprovalDecision {
  decided_by: string
  decision_note?: string
}

export function useApproveBusApproval() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({
      namespace,
      tenant,
      id,
      decision,
    }: {
      namespace: string
      tenant: string
      id: string
      decision: BusApprovalDecision
    }) =>
      apiPost(
        `/v1/bus/approvals/${encodeURIComponent(namespace)}/${encodeURIComponent(tenant)}/${encodeURIComponent(id)}/approve`,
        decision,
      ),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['bus', 'approvals'] }),
  })
}

export function useRejectBusApproval() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({
      namespace,
      tenant,
      id,
      decision,
    }: {
      namespace: string
      tenant: string
      id: string
      decision: BusApprovalDecision
    }) =>
      apiPost(
        `/v1/bus/approvals/${encodeURIComponent(namespace)}/${encodeURIComponent(tenant)}/${encodeURIComponent(id)}/reject`,
        decision,
      ),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['bus', 'approvals'] }),
  })
}
