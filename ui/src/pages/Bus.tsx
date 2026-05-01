// Phase 7 — Agentic Bus admin surface.
//
// Single page with tabs across the bus primitives shipped in
// Phases 1–6c: Topics, Subscriptions (with per-partition lag),
// Agents (heartbeat), Conversations (drilldown link), and
// pre-publish HITL Approvals. The bus is conceptually one feature;
// keeping it on one route avoids fan-out in the sidebar and lets
// operators jump between, say, the conversation thread and the
// matching approval row without losing namespace/tenant filters.

import { useEffect, useMemo, useState } from 'react'
import { useSearchParams, useNavigate } from 'react-router-dom'
import { createColumnHelper } from '@tanstack/react-table'
import { ShieldCheck, Trash2 } from 'lucide-react'

import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Tabs } from '../components/ui/Tabs'
import { EmptyState } from '../components/ui/EmptyState'
import { useToast } from '../components/ui/useToast'
import { relativeTime, formatCountdown } from '../lib/format'
import {
  useBusTopics,
  useBusSubscriptions,
  useBusAgents,
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
            const t = i.row.original
            if (!confirm(`Delete topic ${t.kafka_name}? Kafka data is removed too.`)) return
            del.mutate(
              { namespace: t.namespace, tenant: t.tenant, name: t.name },
              {
                onSuccess: () => toast('success', 'Topic deleted'),
                onError: (err) => toast('error', 'Delete failed', (err as Error).message),
              },
            )
          }}
        >
          <Trash2 className="h-4 w-4" />
        </Button>
      ),
    }),
  ]

  return (
    <DataTable
      data={data ?? []}
      columns={columns}
      loading={isLoading}
      emptyTitle="No bus topics"
      emptyDescription="Create a topic via POST /v1/bus/topics or by deploying an agent that registers an inbox."
    />
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
              const s = i.row.original
              if (!confirm(`Delete subscription ${s.id}?`)) return
              del.mutate(s.id, {
                onSuccess: () => toast('success', 'Subscription deleted'),
                onError: (err) => toast('error', 'Delete failed', (err as Error).message),
              })
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
  const { data, isLoading } = useBusAgents({ namespace: ns || undefined, tenant: tenant || undefined })
  const del = useDeleteBusAgent()
  const { toast } = useToast()

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
      id: 'heartbeat',
      header: 'Heartbeat',
      cell: (i) => <Heartbeat row={i.row.original} />,
    }),
    agentCol.display({
      id: 'actions',
      header: '',
      cell: (i) => (
        <Button
          variant="danger"
          size="sm"
          onClick={(e) => {
            e.stopPropagation()
            const a = i.row.original
            if (!confirm(`Delete agent ${a.agent_id}?`)) return
            del.mutate(
              { namespace: a.namespace, tenant: a.tenant, agentId: a.agent_id },
              {
                onSuccess: () => toast('success', 'Agent deleted'),
                onError: (err) => toast('error', 'Delete failed', (err as Error).message),
              },
            )
          }}
        >
          <Trash2 className="h-4 w-4" />
        </Button>
      ),
    }),
  ]

  return (
    <DataTable
      data={data ?? []}
      columns={columns}
      loading={isLoading}
      emptyTitle="No agents"
      emptyDescription="Agents register themselves via POST /v1/bus/agents and renew via PATCH /heartbeat."
    />
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
  const [statusFilter, setStatusFilter] = useState<BusApprovalStatus | ''>('pending')
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
  onApprove,
  onReject,
}: {
  approval: BusApprovalView
  onApprove: (decided_by: string, decision_note?: string) => void
  onReject: (decided_by: string, decision_note?: string) => void
}) {
  const [decidedBy, setDecidedBy] = useState('')
  const [note, setNote] = useState('')
  const [countdown, setCountdown] = useState(formatCountdown(approval.expires_at))
  useEffect(() => {
    const t = setInterval(() => setCountdown(formatCountdown(approval.expires_at)), 1000)
    return () => clearInterval(t)
  }, [approval.expires_at])
  const envelopeStr = useMemo(() => JSON.stringify(approval.envelope, null, 2), [approval.envelope])
  const terminal = approval.status !== 'pending'

  return (
    <article aria-label={`Approval ${approval.approval_id.slice(0, 8)}`} className={styles.approvalCard}>
      <div className={styles.cardHeader}>
        <Badge variant={approval.status === 'pending' ? 'warning' : approval.status === 'approved' ? 'success' : 'neutral'}>
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
      {!terminal && (
        <>
          <div className={styles.decisionForm}>
            <Input
              placeholder="decided_by (operator id)"
              value={decidedBy}
              onChange={(e) => setDecidedBy(e.target.value)}
            />
            <Input
              placeholder="decision_note (optional)"
              value={note}
              onChange={(e) => setNote(e.target.value)}
            />
          </div>
          <div className={styles.actionButtons}>
            <Button
              variant="danger"
              size="md"
              disabled={!decidedBy}
              onClick={() => onReject(decidedBy, note || undefined)}
            >
              Reject
            </Button>
            <Button
              variant="success"
              size="md"
              disabled={!decidedBy}
              onClick={() => onApprove(decidedBy, note || undefined)}
            >
              Approve
            </Button>
          </div>
        </>
      )}
    </article>
  )
}
