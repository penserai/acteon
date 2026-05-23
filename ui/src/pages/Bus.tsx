// Phase 7 — Agentic Bus admin surface.
//
// Single page with tabs across the bus primitives shipped in
// Phases 1–6c: Topics, Subscriptions (with per-partition lag),
// Agents (heartbeat), Conversations (drilldown link), and
// pre-publish HITL Approvals. The bus is conceptually one feature;
// keeping it on one route avoids fan-out in the sidebar and lets
// operators jump between, say, the conversation thread and the
// matching approval row without losing namespace/tenant filters.

import { useEffect, useMemo, useRef, useState } from 'react'
import { useSearchParams, useNavigate } from 'react-router-dom'
import { createColumnHelper } from '@tanstack/react-table'
import { Ban, Pause, Play, ShieldCheck, Trash2 } from 'lucide-react'

import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Tabs } from '../components/ui/Tabs'
import { EmptyState } from '../components/ui/EmptyState'
import { DeleteConfirmModal } from '../components/ui/DeleteConfirmModal'
import { Modal } from '../components/ui/Modal'
import { useToast } from '../components/ui/useToast'
import { relativeTime, formatCountdown } from '../lib/format'
import {
  useBusTopics,
  useBusSubscriptions,
  useBusAgents,
  useSetBusAgentAdminState,
  useBusConversations,
  useBusApprovals,
  useBusSubscriptionLag,
  useDeleteBusTopic,
  useDeleteBusSubscription,
  useDeleteBusAgent,
  useApproveBusApproval,
  useRejectBusApproval,
  type BusTopic,
  type BusSubscription,
  type BusAgent,
  type SetBusAgentAdminState,
  type BusConversation,
  type BusApprovalStatus,
  type BusApprovalView,
} from '../api/hooks/useBus'

import shared from '../styles/shared.module.css'
import styles from './Bus.module.css'

type TabId = 'topics' | 'subscriptions' | 'agents' | 'conversations' | 'approvals'

const TABS: { id: TabId; label: string }[] = [
  { id: 'topics', label: 'Topics' },
  { id: 'subscriptions', label: 'Subscriptions' },
  { id: 'agents', label: 'Agents' },
  { id: 'conversations', label: 'Conversations' },
  { id: 'approvals', label: 'Approvals' },
]

export function Bus() {
  const [searchParams, setSearchParams] = useSearchParams()
  const tab = (searchParams.get('tab') as TabId) ?? 'topics'
  const ns = searchParams.get('ns') ?? ''
  const tenant = searchParams.get('tenant') ?? ''

  const setTab = (id: string) => {
    const next = new URLSearchParams(searchParams)
    next.set('tab', id)
    setSearchParams(next)
  }
  const setFilter = (key: 'ns' | 'tenant', value: string) => {
    const next = new URLSearchParams(searchParams)
    if (value) next.set(key, value)
    else next.delete(key)
    setSearchParams(next)
  }

  return (
    <div>
      <PageHeader title="Agentic Bus" subtitle="Topics, subscriptions, agents, conversations, and HITL approvals." />

      <div className={styles.filterBar}>
        <Input
          placeholder="Namespace"
          value={ns}
          onChange={(e) => setFilter('ns', e.target.value)}
        />
        <Input
          placeholder="Tenant"
          value={tenant}
          onChange={(e) => setFilter('tenant', e.target.value)}
        />
      </div>

      <Tabs tabs={TABS} active={tab} onChange={setTab} />
      <div className="mt-4">
        {tab === 'topics' && <TopicsPanel ns={ns} tenant={tenant} />}
        {tab === 'subscriptions' && <SubscriptionsPanel ns={ns} tenant={tenant} />}
        {tab === 'agents' && <AgentsPanel ns={ns} tenant={tenant} />}
        {tab === 'conversations' && <ConversationsPanel ns={ns} tenant={tenant} />}
        {tab === 'approvals' && <ApprovalsPanel ns={ns} tenant={tenant} />}
      </div>
    </div>
  )
}

// --------------- Topics ---------------

const topicCol = createColumnHelper<BusTopic>()

function TopicsPanel({ ns, tenant }: { ns: string; tenant: string }) {
  const { data, isLoading } = useBusTopics({ namespace: ns || undefined, tenant: tenant || undefined })
  const del = useDeleteBusTopic()
  const { toast } = useToast()
  const [pending, setPending] = useState<BusTopic | null>(null)

  const columns = [
    topicCol.accessor('kafka_name', { header: 'Kafka topic', cell: (i) => <span className={styles.idCell}>{i.getValue()}</span> }),
    topicCol.accessor('namespace', { header: 'Namespace' }),
    topicCol.accessor('tenant', { header: 'Tenant' }),
    topicCol.accessor('partitions', { header: 'Partitions' }),
    topicCol.accessor('replication_factor', { header: 'Replicas' }),
    topicCol.display({
      id: 'schema',
      header: 'Schema',
      cell: (i) => {
        const t = i.row.original
        return t.schema_subject ? (
          <span className={styles.idCell}>{t.schema_subject} v{t.schema_version}</span>
        ) : (
          <span className={styles.timestampCell}>—</span>
        )
      },
    }),
    topicCol.accessor('created_at', { header: 'Created', cell: (i) => <span className={styles.timestampCell}>{relativeTime(i.getValue())}</span> }),
    topicCol.display({
      id: 'actions',
      header: '',
      cell: (i) => (
        <Button
          variant="danger"
          size="sm"
          onClick={(e) => {
            e.stopPropagation()
            setPending(i.row.original)
          }}
        >
          <Trash2 className="h-4 w-4" />
        </Button>
      ),
    }),
  ]

  return (
    <>
      <DataTable
        data={data ?? []}
        columns={columns}
        loading={isLoading}
        emptyTitle="No bus topics"
        emptyDescription="Create a topic via POST /v1/bus/topics or by deploying an agent that registers an inbox."
      />
      <DeleteConfirmModal
        open={!!pending}
        onClose={() => setPending(null)}
        loading={del.isPending}
        title="Delete bus topic"
        name={pending?.kafka_name ?? ''}
        warning="The backing Kafka topic and all its data are removed too. This cannot be undone."
        onConfirm={() => {
          if (!pending) return
          del.mutate(
            { namespace: pending.namespace, tenant: pending.tenant, name: pending.name },
            {
              onSuccess: () => {
                toast('success', 'Topic deleted')
                setPending(null)
              },
              onError: (err) => toast('error', 'Delete failed', (err as Error).message),
            },
          )
        }}
      />
    </>
  )
}

// --------------- Subscriptions ---------------

const subCol = createColumnHelper<BusSubscription>()

function SubscriptionsPanel({ ns, tenant }: { ns: string; tenant: string }) {
  const { data, isLoading } = useBusSubscriptions({
    namespace: ns || undefined,
    tenant: tenant || undefined,
  })
  const del = useDeleteBusSubscription()
  const { toast } = useToast()
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [pending, setPending] = useState<BusSubscription | null>(null)

  const columns = [
    subCol.accessor('id', { header: 'ID', cell: (i) => <span className={styles.idCell}>{i.getValue()}</span> }),
    subCol.accessor('topic', { header: 'Topic', cell: (i) => <span className={styles.idCell}>{i.getValue()}</span> }),
    subCol.accessor('starting_offset', { header: 'From' }),
    subCol.accessor('ack_mode', { header: 'Ack mode' }),
    subCol.accessor('dead_letter_topic', {
      header: 'DLQ',
      cell: (i) => (i.getValue() ? <span className={styles.idCell}>{i.getValue() as string}</span> : <span className={styles.timestampCell}>—</span>),
    }),
    subCol.display({
      id: 'actions',
      header: '',
      cell: (i) => (
        <div className={styles.actionButtons}>
          <Button
            variant="secondary"
            size="sm"
            onClick={(e) => {
              e.stopPropagation()
              setExpandedId(expandedId === i.row.original.id ? null : i.row.original.id)
            }}
          >
            {expandedId === i.row.original.id ? 'Hide lag' : 'Show lag'}
          </Button>
          <Button
            variant="danger"
            size="sm"
            onClick={(e) => {
              e.stopPropagation()
              setPending(i.row.original)
            }}
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      ),
    }),
  ]

  return (
    <>
      <DataTable
        data={data ?? []}
        columns={columns}
        loading={isLoading}
        emptyTitle="No subscriptions"
        emptyDescription="A subscription is the consumer-group identity for a bus consumer. Create one via POST /v1/bus/subscriptions."
      />
      {expandedId && <SubscriptionLagPanel id={expandedId} />}
      <DeleteConfirmModal
        open={!!pending}
        onClose={() => setPending(null)}
        loading={del.isPending}
        title="Delete subscription"
        name={pending?.id ?? ''}
        warning="The consumer group is removed; in-flight offsets are lost."
        onConfirm={() => {
          if (!pending) return
          del.mutate(pending.id, {
            onSuccess: () => {
              toast('success', 'Subscription deleted')
              setPending(null)
            },
            onError: (err) => toast('error', 'Delete failed', (err as Error).message),
          })
        }}
      />
    </>
  )
}

function SubscriptionLagPanel({ id }: { id: string }) {
  const { data, isLoading } = useBusSubscriptionLag(id)
  if (isLoading) return <div className="mt-4 text-sm text-gray-500">Loading lag…</div>
  if (!data) return null
  return (
    <section className="mt-4 border rounded-lg p-4" style={{ background: 'var(--bg-surface)', borderColor: 'var(--border-default)' }}>
      <h3 className="text-sm font-semibold mb-2">
        {id} — total lag <LagPill lag={data.total_lag} />
      </h3>
      <table className="w-full text-sm">
        <thead>
          <tr className="text-left text-gray-500 text-xs">
            <th className="py-1">Partition</th>
            <th className="py-1">Committed</th>
            <th className="py-1">High water mark</th>
            <th className="py-1">Lag</th>
          </tr>
        </thead>
        <tbody>
          {data.partitions.map((p) => (
            <tr key={p.partition} className="border-t" style={{ borderColor: 'var(--border-default)' }}>
              <td className="py-1">{p.partition}</td>
              <td className="py-1">{p.committed}</td>
              <td className="py-1">{p.high_water_mark}</td>
              <td className="py-1"><LagPill lag={p.lag} /></td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  )
}

function LagPill({ lag }: { lag: number }) {
  // Coarse traffic-light. Operators want "is this consumer falling
  // behind?" at a glance — exact thresholds are tunable later.
  const cls = lag === 0 ? styles.lagOk : lag < 1000 ? styles.lagWarn : styles.lagError
  return <span className={`${styles.lagPill} ${cls}`}>{lag.toLocaleString()}</span>
}

// --------------- Agents ---------------

const agentCol = createColumnHelper<BusAgent>()

function AgentsPanel({ ns, tenant }: { ns: string; tenant: string }) {
  // Admin-state filter is local UI state — passed through as a
  // query param so the server's /v1/bus/agents handler does the
  // filtering rather than the browser. An empty string means "all
  // states" (the absence of the param to the server).
  const [adminFilter, setAdminFilter] = useState<'' | 'active' | 'suspended' | 'banned'>('')
  const { data, isLoading } = useBusAgents({
    namespace: ns || undefined,
    tenant: tenant || undefined,
    admin_state: adminFilter || undefined,
  })
  const del = useDeleteBusAgent()
  const { toast } = useToast()
  const [pending, setPending] = useState<BusAgent | null>(null)
  const [adminTarget, setAdminTarget] = useState<BusAgent | null>(null)

  const columns = [
    agentCol.accessor('agent_id', { header: 'Agent', cell: (i) => <span className={styles.idCell}>{i.getValue()}</span> }),
    agentCol.accessor('namespace', { header: 'Namespace' }),
    agentCol.accessor('tenant', { header: 'Tenant' }),
    agentCol.accessor('inbox_topic', { header: 'Inbox', cell: (i) => <span className={styles.idCell}>{i.getValue()}</span> }),
    agentCol.accessor('capabilities', {
      header: 'Capabilities',
      cell: (i) => (i.getValue() ?? []).map((c) => <Badge key={c}>{c}</Badge>),
    }),
    agentCol.accessor('status', { header: 'Status', cell: (i) => <Badge>{i.getValue()}</Badge> }),
    agentCol.display({
      id: 'admin_state',
      header: 'Admin',
      cell: (i) => <AdminStateBadge row={i.row.original} />,
    }),
    agentCol.display({
      id: 'heartbeat',
      header: 'Heartbeat',
      cell: (i) => <Heartbeat row={i.row.original} />,
    }),
    agentCol.display({
      id: 'actions',
      header: '',
      cell: (i) => (
        <div style={{ display: 'flex', gap: '0.25rem' }}>
          <Button
            variant="secondary"
            size="sm"
            title="Set admin state"
            onClick={(e) => {
              e.stopPropagation()
              setAdminTarget(i.row.original)
            }}
          >
            <ShieldCheck className="h-4 w-4" />
          </Button>
          <Button
            variant="danger"
            size="sm"
            onClick={(e) => {
              e.stopPropagation()
              setPending(i.row.original)
            }}
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      ),
    }),
  ]

  return (
    <>
      <div style={{ display: 'flex', gap: '0.5rem', alignItems: 'center', marginBottom: '0.75rem' }}>
        <label style={{ fontSize: '0.875rem', color: 'var(--color-fg-muted, #6b7280)' }}>
          Admin state:
        </label>
        <select
          value={adminFilter}
          onChange={(e) => setAdminFilter(e.target.value as typeof adminFilter)}
          style={{
            padding: '0.25rem 0.5rem',
            borderRadius: '0.375rem',
            border: '1px solid var(--color-border, #d1d5db)',
            background: 'var(--color-bg, #fff)',
          }}
        >
          <option value="">All</option>
          <option value="active">Active</option>
          <option value="suspended">Suspended</option>
          <option value="banned">Banned</option>
        </select>
      </div>
      <DataTable
        data={data ?? []}
        columns={columns}
        loading={isLoading}
        emptyTitle="No agents"
        emptyDescription="Agents register themselves via POST /v1/bus/agents and renew via PATCH /heartbeat."
      />
      <DeleteConfirmModal
        open={!!pending}
        onClose={() => setPending(null)}
        loading={del.isPending}
        title="Delete agent"
        name={pending?.agent_id ?? ''}
        warning="The agent's identity row and inbox binding are removed; an agent re-registering with the same id is a fresh identity."
        onConfirm={() => {
          if (!pending) return
          del.mutate(
            { namespace: pending.namespace, tenant: pending.tenant, agentId: pending.agent_id },
            {
              onSuccess: () => {
                toast('success', 'Agent deleted')
                setPending(null)
              },
              onError: (err) => toast('error', 'Delete failed', (err as Error).message),
            },
          )
        }}
      />
      {/*
        The `key` forces a fresh AdminStateModal instance per agent
        so its internal useState defaults pick up the new agent's
        admin_state / admin_reason on open, with no effect-driven
        sync (the React Compiler rejects `setState` inside
        useEffect).
      */}
      {adminTarget && (
        <AdminStateModal
          key={`${adminTarget.namespace}/${adminTarget.tenant}/${adminTarget.agent_id}`}
          agent={adminTarget}
          onClose={() => setAdminTarget(null)}
        />
      )}
    </>
  )
}

/**
 * Tiny coloured badge for the admin-state column. Active = neutral
 * (it's the boring default and shouldn't dominate the row);
 * Suspended = warning; Banned = danger.
 */
function AdminStateBadge({ row }: { row: BusAgent }) {
  const state = row.admin_state ?? 'active'
  const colors: Record<string, { bg: string; fg: string; ring: string }> = {
    active: { bg: '#f3f4f6', fg: '#374151', ring: '#d1d5db' },
    suspended: { bg: '#fef3c7', fg: '#92400e', ring: '#fcd34d' },
    banned: { bg: '#fee2e2', fg: '#991b1b', ring: '#fca5a5' },
  }
  const c = colors[state] ?? colors.active
  return (
    <span
      title={row.admin_reason ?? undefined}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        padding: '0.125rem 0.5rem',
        borderRadius: '9999px',
        background: c.bg,
        color: c.fg,
        boxShadow: `inset 0 0 0 1px ${c.ring}`,
        fontSize: '0.75rem',
        fontWeight: 500,
      }}
    >
      {state}
    </span>
  )
}

/**
 * Admin-state mutation modal. Three buttons (one per state). When
 * Suspended is chosen, the operator can also set a duration —
 * server stamps the expiry and auto-reinstates on read past it.
 *
 * Banned is one-shot from the UI: we don't expose an `expires_at`
 * field for it because the server drops the expiry on Banned and a
 * banned-with-expiry UI would imply auto-reinstate, which is
 * intentionally not how bans work.
 */
// The parent always wraps this in `{agent && <AdminStateModal …
// key=… />}` so the component only mounts when an agent is
// selected — every useState default below evaluates exactly once
// per (agent, open) pair. That keeps us out of the React Compiler's
// "no setState in useEffect" rule.
function AdminStateModal({ agent, onClose }: { agent: BusAgent; onClose: () => void }) {
  const mut = useSetBusAgentAdminState()
  const { toast } = useToast()
  const [target, setTarget] = useState<'active' | 'suspended' | 'banned'>(
    (agent.admin_state as 'active' | 'suspended' | 'banned' | undefined) ?? 'active',
  )
  const [reason, setReason] = useState(agent.admin_reason ?? '')
  // Suspension duration in minutes. 0 = no expiry (operator must
  // manually reinstate).
  const [durationMin, setDurationMin] = useState(60)

  const submit = () => {
    const body: SetBusAgentAdminState = { admin_state: target }
    if (reason.trim()) body.reason = reason.trim()
    if (target === 'suspended' && durationMin > 0) {
      body.expires_at = new Date(Date.now() + durationMin * 60_000).toISOString()
    }
    mut.mutate(
      { namespace: agent.namespace, tenant: agent.tenant, agentId: agent.agent_id, body },
      {
        onSuccess: () => {
          toast('success', `Agent set to ${target}`)
          onClose()
        },
        onError: (err) => toast('error', 'Admin state change failed', (err as Error).message),
      },
    )
  }

  return (
    <Modal
      open={!!agent}
      onClose={onClose}
      title={`Admin state — ${agent.agent_id}`}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={mut.isPending}>
            Cancel
          </Button>
          <Button onClick={submit} disabled={mut.isPending}>
            {mut.isPending ? 'Applying…' : 'Apply'}
          </Button>
        </>
      }
    >
      <div style={{ display: 'flex', flexDirection: 'column', gap: '1rem' }}>
        <div>
          <div style={{ fontSize: '0.875rem', color: 'var(--color-fg-muted, #6b7280)', marginBottom: '0.5rem' }}>
            Current state: <strong>{agent.admin_state ?? 'active'}</strong>
            {agent.admin_set_by && (
              <span> (set by {agent.admin_set_by})</span>
            )}
          </div>
          <div style={{ display: 'flex', gap: '0.5rem' }}>
            <StateChoice
              icon={<Play className="h-4 w-4" />}
              label="Active"
              value="active"
              selected={target}
              onSelect={setTarget}
            />
            <StateChoice
              icon={<Pause className="h-4 w-4" />}
              label="Suspended"
              value="suspended"
              selected={target}
              onSelect={setTarget}
            />
            <StateChoice
              icon={<Ban className="h-4 w-4" />}
              label="Banned"
              value="banned"
              selected={target}
              onSelect={setTarget}
            />
          </div>
        </div>

        {target === 'suspended' && (
          <div>
            <label style={{ display: 'block', fontSize: '0.875rem', marginBottom: '0.25rem' }}>
              Auto-reinstate after (minutes, 0 = no expiry)
            </label>
            <input
              type="number"
              min={0}
              value={durationMin}
              onChange={(e) => setDurationMin(Math.max(0, Number(e.target.value) || 0))}
              style={{
                width: '100%',
                padding: '0.375rem 0.5rem',
                borderRadius: '0.375rem',
                border: '1px solid var(--color-border, #d1d5db)',
              }}
            />
          </div>
        )}

        <div>
          <label style={{ display: 'block', fontSize: '0.875rem', marginBottom: '0.25rem' }}>
            Reason (surfaced on the 403 to callers; keep terse)
          </label>
          <textarea
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            rows={3}
            maxLength={4096}
            placeholder={target === 'banned' ? 'e.g. exfiltration attempt' : 'e.g. flaky retries'}
            style={{
              width: '100%',
              padding: '0.375rem 0.5rem',
              borderRadius: '0.375rem',
              border: '1px solid var(--color-border, #d1d5db)',
              fontFamily: 'inherit',
              resize: 'vertical',
            }}
          />
        </div>
      </div>
    </Modal>
  )
}

function StateChoice({
  icon,
  label,
  value,
  selected,
  onSelect,
}: {
  icon: React.ReactNode
  label: string
  value: 'active' | 'suspended' | 'banned'
  selected: string
  onSelect: (v: 'active' | 'suspended' | 'banned') => void
}) {
  const isSelected = selected === value
  return (
    <button
      type="button"
      onClick={() => onSelect(value)}
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: '0.25rem',
        flex: 1,
        padding: '0.75rem',
        borderRadius: '0.5rem',
        border: `1px solid ${isSelected ? 'var(--color-primary, #2563eb)' : 'var(--color-border, #d1d5db)'}`,
        background: isSelected ? 'var(--color-primary-soft, #dbeafe)' : 'transparent',
        cursor: 'pointer',
        fontSize: '0.875rem',
      }}
    >
      {icon}
      <span>{label}</span>
    </button>
  )
}

function Heartbeat({ row }: { row: BusAgent }) {
  // Re-render every second so the relative timestamp stays fresh
  // without forcing a full re-fetch. The query already polls every
  // 5s; this is just the in-place countdown UX. `now` lives in
  // state instead of a direct `Date.now()` call in render so the
  // React Compiler purity rule is satisfied.
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [])
  if (!row.last_heartbeat_at) return <span className={styles.timestampCell}>never</span>
  const ageMs = now - new Date(row.last_heartbeat_at).getTime()
  const stale = ageMs > row.heartbeat_ttl_ms
  return (
    <span className={`${styles.timestampCell} ${stale ? styles.heartbeatStale : styles.heartbeatHealthy}`}>
      {relativeTime(row.last_heartbeat_at)}
      {stale && ' (stale)'}
    </span>
  )
}

// --------------- Conversations ---------------

const convCol = createColumnHelper<BusConversation>()

function ConversationsPanel({ ns, tenant }: { ns: string; tenant: string }) {
  const navigate = useNavigate()
  const { data, isLoading } = useBusConversations({
    namespace: ns || undefined,
    tenant: tenant || undefined,
  })

  const columns = [
    convCol.accessor('conversation_id', {
      header: 'Conversation',
      cell: (i) => <span className={styles.idCell}>{i.getValue()}</span>,
    }),
    convCol.accessor('state', { header: 'State', cell: (i) => <Badge>{i.getValue()}</Badge> }),
    convCol.accessor('participants', {
      header: 'Participants',
      cell: (i) => {
        const ps = i.getValue() ?? []
        if (ps.length === 0) return <span className={styles.timestampCell}>open</span>
        return ps.slice(0, 3).map((p) => <Badge key={p}>{p}</Badge>)
      },
    }),
    convCol.accessor('events_topic', {
      header: 'Events topic',
      cell: (i) => i.getValue() ? <span className={styles.idCell}>{i.getValue() as string}</span> : <span className={styles.timestampCell}>(default)</span>,
    }),
    convCol.accessor('updated_at', { header: 'Updated', cell: (i) => <span className={styles.timestampCell}>{relativeTime(i.getValue())}</span> }),
  ]

  return (
    <DataTable
      data={data ?? []}
      columns={columns}
      loading={isLoading}
      onRowClick={(row) =>
        navigate(
          `/bus/conversations/${encodeURIComponent(row.namespace)}/${encodeURIComponent(row.tenant)}/${encodeURIComponent(row.conversation_id)}`,
        )
      }
      emptyTitle="No conversations"
      emptyDescription="Conversations are the bus thread primitive. Create one via POST /v1/bus/conversations."
    />
  )
}

// --------------- Approvals ---------------

function ApprovalsPanel({ ns, tenant }: { ns: string; tenant: string }) {
  const [searchParams, setSearchParams] = useSearchParams()
  const focusId = searchParams.get('approval_id') ?? ''
  // Default the status filter to "all" when deep-linked so the
  // target row is visible regardless of its current state — a
  // conversation thread frequently links to an already-approved
  // record for audit, not a still-pending one.
  const [statusFilter, setStatusFilter] = useState<BusApprovalStatus | ''>(
    focusId ? '' : 'pending',
  )
  const { data, isLoading } = useBusApprovals(ns || undefined, tenant || undefined, {
    status: statusFilter || undefined,
  })
  const approve = useApproveBusApproval()
  const reject = useRejectBusApproval()
  const { toast } = useToast()

  // Approvals are tenant-scoped — without ns/tenant the list endpoint
  // returns 400; surface that explicitly so operators understand why
  // the panel is empty.
  if (!ns || !tenant) {
    return (
      <EmptyState
        icon={<ShieldCheck className="h-12 w-12" />}
        title="Pick a namespace + tenant"
        description="Bus approvals are scoped per (namespace, tenant). Use the filters above to load the queue."
      />
    )
  }

  const approvals = data?.approvals ?? []

  return (
    <div>
      <div className="mb-4">
        <Select
          options={[
            { value: 'pending', label: 'Pending' },
            { value: 'approving', label: 'Approving (mid-flight)' },
            { value: 'approved', label: 'Approved' },
            { value: 'rejected', label: 'Rejected' },
            { value: 'expired', label: 'Expired' },
            { value: '', label: 'All' },
          ]}
          value={statusFilter}
          onChange={(e) => setStatusFilter(e.target.value as BusApprovalStatus | '')}
        />
      </div>
      {isLoading ? (
        <div className={styles.timestampCell}>Loading approvals…</div>
      ) : approvals.length === 0 ? (
        <EmptyState
          icon={<ShieldCheck className="h-12 w-12" />}
          title={statusFilter ? `No ${statusFilter} approvals` : 'No approvals'}
          description="Bus approvals are created when a tool-call POST sets require_approval=true."
        />
      ) : (
        <div className={styles.approvalsList}>
          {approvals.map((a) => (
            <BusApprovalCard
              key={a.approval_id}
              approval={a}
              focused={a.approval_id === focusId}
              onFocusConsumed={() => {
                // Clear the deep-link param after the card has
                // mounted + scrolled. Operators can refresh without
                // re-pulsing the same row, and the URL stays clean
                // for sharing.
                const next = new URLSearchParams(searchParams)
                next.delete('approval_id')
                setSearchParams(next, { replace: true })
              }}
              onApprove={(decided_by, decision_note) =>
                approve.mutate(
                  { namespace: ns, tenant, id: a.approval_id, decision: { decided_by, decision_note } },
                  {
                    onSuccess: () => toast('success', 'Approved'),
                    onError: (err) => toast('error', 'Approve failed', (err as Error).message),
                  },
                )
              }
              onReject={(decided_by, decision_note) =>
                reject.mutate(
                  { namespace: ns, tenant, id: a.approval_id, decision: { decided_by, decision_note } },
                  {
                    onSuccess: () => toast('success', 'Rejected'),
                    onError: (err) => toast('error', 'Reject failed', (err as Error).message),
                  },
                )
              }
            />
          ))}
        </div>
      )}
    </div>
  )
}

function BusApprovalCard({
  approval,
  focused,
  onFocusConsumed,
  onApprove,
  onReject,
}: {
  approval: BusApprovalView
  focused?: boolean
  onFocusConsumed?: () => void
  onApprove: (decided_by: string, decision_note?: string) => void
  onReject: (decided_by: string, decision_note?: string) => void
}) {
  const [decidedBy, setDecidedBy] = useState('')
  const [note, setNote] = useState('')
  const [countdown, setCountdown] = useState(formatCountdown(approval.expires_at))
  const cardRef = useRef<HTMLElement>(null)
  useEffect(() => {
    const t = setInterval(() => setCountdown(formatCountdown(approval.expires_at)), 1000)
    return () => clearInterval(t)
  }, [approval.expires_at])
  // Scroll the deep-linked card into view on mount and let the
  // parent clear the URL param so the highlight only fires once.
  useEffect(() => {
    if (focused && cardRef.current) {
      cardRef.current.scrollIntoView({ behavior: 'smooth', block: 'center' })
      onFocusConsumed?.()
    }
  }, [focused, onFocusConsumed])
  const envelopeStr = useMemo(() => JSON.stringify(approval.envelope, null, 2), [approval.envelope])
  // Pending and Approving both expose decision controls — Pending
  // is the first decision; Approving is the manual retry path
  // (the produce failed mid-flight; calling approve again retries
  // it without overwriting the original decided_by).
  const decidable = approval.status === 'pending' || approval.status === 'approving'
  const isRetrying = approval.status === 'approving'

  return (
    <article
      ref={cardRef}
      aria-label={`Approval ${approval.approval_id.slice(0, 8)}`}
      className={`${styles.approvalCard} ${focused ? styles.approvalCardHighlighted : ''}`}
    >
      <div className={styles.cardHeader}>
        <Badge
          variant={
            approval.status === 'pending'
              ? 'warning'
              : approval.status === 'approving'
                // Approving = operator decided, produce mid-flight.
                // Surfaced as `info` so it visually distinguishes
                // from still-pending (warning, decision-needed) and
                // from approved (success, fully done).
                ? 'info'
                : approval.status === 'approved'
                  ? 'success'
                  : 'neutral'
          }
        >
          {approval.status}
        </Badge>
        <span className={styles.timestamp}>{relativeTime(approval.created_at)}</span>
      </div>
      <div className={styles.detailsContainer}>
        <p>
          <span className={shared.detailLabel}>ID:</span>{' '}
          <span className={styles.idCell}>{approval.approval_id}</span>
        </p>
        <p>
          <span className={shared.detailLabel}>Conversation:</span>{' '}
          <span className={styles.idCell}>{approval.conversation_id}</span>
        </p>
        <p>
          <span className={shared.detailLabel}>Envelope:</span> {approval.envelope_kind}{' '}
          ({approval.correlation_token})
        </p>
        {approval.reason && (
          <p>
            <span className={shared.detailLabel}>Reason:</span> {approval.reason}
          </p>
        )}
        {approval.decided_by && (
          <p>
            <span className={shared.detailLabel}>Decided by:</span> {approval.decided_by}
            {approval.decision_note && ` — ${approval.decision_note}`}
          </p>
        )}
        {approval.produced_offset !== null && approval.produced_offset !== undefined && (
          <p>
            <span className={shared.detailLabel}>Produced:</span> partition{' '}
            {approval.produced_partition}, offset {approval.produced_offset}
          </p>
        )}
      </div>
      <details>
        <summary className="cursor-pointer text-xs text-gray-500 mb-2">Show envelope payload</summary>
        <pre className={styles.envelopePreview}>{envelopeStr}</pre>
      </details>
      <div className={styles.metadataRow}>
        <span>Expires: {countdown}</span>
      </div>
      {decidable && (
        <>
          {isRetrying && (
            <p className="text-xs mb-2" style={{ color: 'var(--text-warning, #d97706)' }}>
              Produce failed mid-flight. Click Approve again to retry — the
              original decided_by is preserved; idempotent producer + consumer-
              side dedup on call_id keep the topic clean.
            </p>
          )}
          <div className={styles.decisionForm}>
            <Input
              placeholder={
                isRetrying
                  ? `decided_by (locked: ${approval.decided_by})`
                  : 'decided_by (operator id)'
              }
              value={isRetrying ? approval.decided_by ?? '' : decidedBy}
              onChange={(e) => setDecidedBy(e.target.value)}
              disabled={isRetrying}
            />
            <Input
              placeholder="decision_note (optional)"
              value={note}
              onChange={(e) => setNote(e.target.value)}
            />
          </div>
          <div className={styles.actionButtons}>
            {/* Reject is only valid from Pending — Approving means
                the operator already approved and the produce is
                in flight. The server returns 409 if asked to
                reject from Approving; hide the button to match. */}
            {!isRetrying && (
              <Button
                variant="danger"
                size="md"
                disabled={!decidedBy}
                onClick={() => onReject(decidedBy, note || undefined)}
              >
                Reject
              </Button>
            )}
            <Button
              variant="success"
              size="md"
              disabled={isRetrying ? false : !decidedBy}
              onClick={() =>
                onApprove(
                  isRetrying ? approval.decided_by ?? '' : decidedBy,
                  note || undefined,
                )
              }
            >
              {isRetrying ? 'Retry produce' : 'Approve'}
            </Button>
          </div>
        </>
      )}
    </article>
  )
}
