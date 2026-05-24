// /agents/{namespace}/{tenant}/{agentId}
//
// Agent control plane detail view. Three tabs:
//
//   Overview — identity, capabilities, heartbeat status, admin
//              metadata, plus inline lifecycle actions
//              (Suspend / Reinstate / Ban) wired to the
//              admin-state endpoint.
//   Card     — A2A AgentCard view / edit / delete. Empty state has
//              a Create CTA that primes a minimal valid card.
//   Activity — placeholder pointing at the audit trail for now,
//              with a per-agent prefilter when those filters land.

import { useEffect, useMemo, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import {
  ArrowLeft, Ban, Pencil, Plus, RotateCcw, ShieldOff, Trash2,
} from 'lucide-react'

import {
  useBusAgent,
  useDeleteBusAgent,
  useSetBusAgentAdminState,
  useBusAgentCard,
  useUpsertBusAgentCard,
  useDeleteBusAgentCard,
  type AgentCard,
  type BusAgent,
  type SetBusAgentAdminState,
} from '../api/hooks/useBus'
import { PageHeader } from '../components/layout/PageHeader'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Modal } from '../components/ui/Modal'
import { DeleteConfirmModal } from '../components/ui/DeleteConfirmModal'
import { EmptyState } from '../components/ui/EmptyState'
import { Tabs } from '../components/ui/Tabs'
import { Skeleton } from '../components/ui/Skeleton'
import { useToast } from '../components/ui/useToast'
import { absoluteTime, relativeTime } from '../lib/format'
import shared from '../styles/shared.module.css'
import styles from './Agents.module.css'

function adminVariant(s: string | undefined): 'success' | 'warning' | 'error' {
  switch (s) {
    case 'suspended':
      return 'warning'
    case 'banned':
      return 'error'
    default:
      return 'success'
  }
}

function statusVariant(s: string): 'success' | 'warning' | 'error' | 'neutral' {
  switch (s) {
    case 'online':
      return 'success'
    case 'idle':
      return 'warning'
    case 'dead':
      return 'error'
    default:
      return 'neutral'
  }
}

export function AgentDetail() {
  const { namespace, tenant, agentId } = useParams<{
    namespace: string
    tenant: string
    agentId: string
  }>()
  const navigate = useNavigate()
  const { toast } = useToast()

  const { data: agent, isLoading } = useBusAgent(namespace, tenant, agentId)
  const setAdmin = useSetBusAgentAdminState()
  const del = useDeleteBusAgent()

  const [deleteOpen, setDeleteOpen] = useState(false)
  const [adminModal, setAdminModal] = useState<'suspend' | 'ban' | null>(null)
  const [activeTab, setActiveTab] = useState<'overview' | 'card' | 'activity'>('overview')

  function reinstate() {
    if (!namespace || !tenant || !agentId) return
    setAdmin.mutate(
      { namespace, tenant, agentId, body: { admin_state: 'active' } },
      {
        onSuccess: () => toast('success', `Reinstated ${agentId}`),
        onError: (e) => toast('error', 'Reinstate failed', (e as Error).message),
      },
    )
  }

  function doDelete() {
    if (!namespace || !tenant || !agentId) return
    del.mutate(
      { namespace, tenant, agentId },
      {
        onSuccess: () => {
          toast('success', `Deleted ${agentId}`)
          navigate('/agents')
        },
        onError: (e) => toast('error', 'Delete failed', (e as Error).message),
      },
    )
  }

  if (isLoading) {
    return (
      <div>
        <PageHeader title="Agent" subtitle={`${namespace} / ${tenant}`} />
        <Skeleton className="h-40 w-full" />
      </div>
    )
  }

  if (!agent) {
    return (
      <div>
        <PageHeader title="Agent" subtitle={`${namespace} / ${tenant}`} />
        <EmptyState title="Agent not found" description="The agent record may have been deleted, or you may not have access to it." />
      </div>
    )
  }

  const adminEffective = (agent.admin_state ?? 'active') as 'active' | 'suspended' | 'banned'
  const isActive = adminEffective === 'active'

  return (
    <div>
      <PageHeader
        title={agent.agent_id}
        subtitle={`${agent.namespace} / ${agent.tenant}`}
        actions={
          <div className={styles.headerActions}>
            <Link to="/agents" className={styles.muted}>
              <ArrowLeft className="h-4 w-4 inline" /> All agents
            </Link>
            <Badge variant={adminVariant(adminEffective)}>{adminEffective}</Badge>
            <Badge variant={statusVariant(agent.status)} size="sm">{agent.status}</Badge>
            {isActive && (
              <>
                <Button
                  variant="secondary"
                  size="sm"
                  icon={<ShieldOff className="h-3.5 w-3.5" />}
                  onClick={() => setAdminModal('suspend')}
                >
                  Suspend
                </Button>
                <Button
                  variant="danger"
                  size="sm"
                  icon={<Ban className="h-3.5 w-3.5" />}
                  onClick={() => setAdminModal('ban')}
                >
                  Ban
                </Button>
              </>
            )}
            {!isActive && (
              <Button
                variant="primary"
                size="sm"
                icon={<RotateCcw className="h-3.5 w-3.5" />}
                onClick={reinstate}
                loading={setAdmin.isPending}
              >
                Reinstate
              </Button>
            )}
            <Button
              variant="ghost"
              size="sm"
              icon={<Trash2 className="h-3.5 w-3.5" />}
              onClick={() => setDeleteOpen(true)}
            >
              Delete
            </Button>
          </div>
        }
      />

      <Tabs
        tabs={[
          { id: 'overview', label: 'Overview' },
          { id: 'card', label: 'A2A Card' },
          { id: 'activity', label: 'Activity' },
        ]}
        active={activeTab}
        onChange={(id) => setActiveTab(id as 'overview' | 'card' | 'activity')}
      />
      {activeTab === 'overview' && <OverviewPanel agent={agent} />}
      {activeTab === 'card' && (
        <CardPanel
          namespace={agent.namespace}
          tenant={agent.tenant}
          agentId={agent.agent_id}
          agentName={agent.display_name ?? agent.agent_id}
        />
      )}
      {activeTab === 'activity' && <ActivityPanel agent={agent} />}

      <AdminStateModal
        open={adminModal !== null}
        mode={adminModal}
        onClose={() => setAdminModal(null)}
        onSubmit={(body) => {
          if (!namespace || !tenant || !agentId) return
          setAdmin.mutate(
            { namespace, tenant, agentId, body },
            {
              onSuccess: () => {
                toast('success', `${body.admin_state} ${agentId}`)
                setAdminModal(null)
              },
              onError: (e) => toast('error', 'Admin state failed', (e as Error).message),
            },
          )
        }}
        loading={setAdmin.isPending}
      />

      <DeleteConfirmModal
        open={deleteOpen}
        onClose={() => setDeleteOpen(false)}
        onConfirm={doDelete}
        loading={del.isPending}
        title="Delete agent"
        name={agent.agent_id}
        warning="Deletes the agent record entirely. The audit metadata on this row will be lost — prefer Ban if you want to keep the audit trail."
      />
    </div>
  )
}

// ---- Overview tab ----

function OverviewPanel({ agent }: { agent: BusAgent }) {
  return (
    <div className={styles.section}>
      <h3 className={styles.sectionTitle}>Identity</h3>
      <dl className={styles.metaGrid}>
        <dt className={styles.metaLabel}>agent_id</dt><dd className={styles.metaValue}>{agent.agent_id}</dd>
        <dt className={styles.metaLabel}>namespace / tenant</dt><dd className={styles.metaValue}>{agent.namespace} / {agent.tenant}</dd>
        <dt className={styles.metaLabel}>inbox topic</dt><dd className={styles.metaValue}>{agent.inbox_topic}</dd>
        <dt className={styles.metaLabel}>heartbeat TTL</dt><dd className={styles.metaValue}>{agent.heartbeat_ttl_ms} ms</dd>
        <dt className={styles.metaLabel}>last heartbeat</dt>
        <dd className={styles.metaValue}>
          {agent.last_heartbeat_at ? `${absoluteTime(agent.last_heartbeat_at)} (${relativeTime(agent.last_heartbeat_at)})` : 'never'}
        </dd>
        <dt className={styles.metaLabel}>created</dt><dd className={styles.metaValue}>{absoluteTime(agent.created_at)}</dd>
        <dt className={styles.metaLabel}>updated</dt><dd className={styles.metaValue}>{absoluteTime(agent.updated_at)}</dd>
      </dl>

      <div className={styles.section}>
        <h3 className={styles.sectionTitle}>Capabilities</h3>
        {agent.capabilities.length === 0 ? (
          <span className={styles.muted}>None declared.</span>
        ) : (
          <div className={styles.badgeRow}>
            {agent.capabilities.map((c) => (
              <Badge key={c} variant="info" size="sm">{c}</Badge>
            ))}
          </div>
        )}
      </div>

      {agent.admin_state && agent.admin_state !== 'active' && (
        <div className={styles.section}>
          <h3 className={styles.sectionTitle}>Admin metadata</h3>
          <dl className={styles.metaGrid}>
            <dt className={styles.metaLabel}>state</dt>
            <dd className={styles.metaValue}>
              <Badge variant={adminVariant(agent.admin_state)} size="sm">{agent.admin_state}</Badge>
            </dd>
            {agent.admin_reason && (
              <>
                <dt className={styles.metaLabel}>reason</dt>
                <dd className={styles.metaValue}>{agent.admin_reason}</dd>
              </>
            )}
            {agent.admin_set_by && (
              <>
                <dt className={styles.metaLabel}>set by</dt>
                <dd className={styles.metaValue}>{agent.admin_set_by}</dd>
              </>
            )}
            {agent.admin_set_at && (
              <>
                <dt className={styles.metaLabel}>set at</dt>
                <dd className={styles.metaValue}>{absoluteTime(agent.admin_set_at)}</dd>
              </>
            )}
            {agent.admin_expires_at && (
              <>
                <dt className={styles.metaLabel}>auto-reinstate at</dt>
                <dd className={styles.metaValue}>
                  {absoluteTime(agent.admin_expires_at)} ({relativeTime(agent.admin_expires_at)})
                </dd>
              </>
            )}
          </dl>
        </div>
      )}
    </div>
  )
}

// ---- Activity tab ----

function ActivityPanel({ agent }: { agent: BusAgent }) {
  return (
    <div className={styles.section}>
      <EmptyState
        title="Activity stream coming soon"
        description={`This will surface bus traffic and admin-state changes for ${agent.agent_id}. For now, filter the audit trail by agent_id.`}
      />
      <p className={styles.muted}>
        <Link to={`/audit?agent_id=${encodeURIComponent(agent.agent_id)}`}>Open in Audit Trail →</Link>
      </p>
    </div>
  )
}

// ---- A2A Card tab ----

function CardPanel({
  namespace,
  tenant,
  agentId,
  agentName,
}: {
  namespace: string
  tenant: string
  agentId: string
  agentName: string
}) {
  const { data: card, isLoading } = useBusAgentCard(namespace, tenant, agentId)
  const upsert = useUpsertBusAgentCard()
  const del = useDeleteBusAgentCard()
  const { toast } = useToast()
  const [editorOpen, setEditorOpen] = useState(false)
  const [deleteOpen, setDeleteOpen] = useState(false)

  if (isLoading) {
    return <Skeleton className="h-48 w-full mt-4" />
  }

  return (
    <div className={styles.section}>
      <div className={styles.sectionHeader}>
        <h3 className={styles.sectionTitle}>A2A AgentCard</h3>
        <div className={styles.headerActions}>
          {card ? (
            <>
              <Button
                variant="secondary"
                size="sm"
                icon={<Pencil className="h-3.5 w-3.5" />}
                onClick={() => setEditorOpen(true)}
              >
                Edit JSON
              </Button>
              <Button
                variant="ghost"
                size="sm"
                icon={<Trash2 className="h-3.5 w-3.5" />}
                onClick={() => setDeleteOpen(true)}
              >
                Delete
              </Button>
            </>
          ) : (
            <Button
              variant="primary"
              size="sm"
              icon={<Plus className="h-3.5 w-3.5" />}
              onClick={() => setEditorOpen(true)}
            >
              Create card
            </Button>
          )}
        </div>
      </div>

      {!card ? (
        <EmptyState
          title="No AgentCard published"
          description="An AgentCard makes this agent discoverable via the A2A /.well-known/agent-card endpoint. Create one with the button above."
        />
      ) : (
        <>
          <dl className={styles.metaGrid}>
            <dt className={styles.metaLabel}>name</dt><dd className={styles.metaValue}>{card.name}</dd>
            <dt className={styles.metaLabel}>version</dt><dd className={styles.metaValue}>{card.version}</dd>
            {card.description && (
              <>
                <dt className={styles.metaLabel}>description</dt>
                <dd className={styles.metaValue}>{card.description}</dd>
              </>
            )}
            <dt className={styles.metaLabel}>skills</dt>
            <dd className={styles.metaValue}>
              {(card.skills?.length ?? 0) === 0 ? (
                <span className={styles.muted}>none</span>
              ) : (
                <div className={styles.badgeRow}>
                  {card.skills!.map((s) => (
                    <Badge key={s.id} variant="info" size="sm">
                      {s.name}
                    </Badge>
                  ))}
                </div>
              )}
            </dd>
          </dl>
          <details className={`mt-3`}>
            <summary className={styles.muted}>Raw JSON</summary>
            <pre className={styles.cardJson}>{JSON.stringify(card, null, 2)}</pre>
          </details>
        </>
      )}

      <CardEditorModal
        open={editorOpen}
        onClose={() => setEditorOpen(false)}
        initial={card}
        defaults={{ namespace, tenant, agentId, agentName }}
        loading={upsert.isPending}
        onSubmit={(next) => {
          upsert.mutate(
            { namespace, tenant, agentId, card: next },
            {
              onSuccess: () => {
                toast('success', card ? 'Card updated' : 'Card created')
                setEditorOpen(false)
              },
              onError: (e) => toast('error', 'Save failed', (e as Error).message),
            },
          )
        }}
      />

      <DeleteConfirmModal
        open={deleteOpen}
        onClose={() => setDeleteOpen(false)}
        loading={del.isPending}
        title="Delete AgentCard"
        name={card?.name ?? agentId}
        warning="Removes this agent from A2A discovery. The agent record itself stays — only the card is deleted."
        onConfirm={() =>
          del.mutate(
            { namespace, tenant, agentId },
            {
              onSuccess: () => {
                toast('success', 'Card deleted')
                setDeleteOpen(false)
              },
              onError: (e) => toast('error', 'Delete failed', (e as Error).message),
            },
          )
        }
      />
    </div>
  )
}

// ---- Card JSON editor ----

function CardEditorModal({
  open,
  onClose,
  onSubmit,
  loading,
  initial,
  defaults,
}: {
  open: boolean
  onClose: () => void
  onSubmit: (card: AgentCard) => void
  loading: boolean
  initial: AgentCard | null | undefined
  defaults: { namespace: string; tenant: string; agentId: string; agentName: string }
}) {
  const seed = useMemo<AgentCard>(() => {
    if (initial) return initial
    const now = new Date().toISOString()
    return {
      agentId: defaults.agentId,
      namespace: defaults.namespace,
      tenant: defaults.tenant,
      name: defaults.agentName,
      version: '1.0.0',
      description: '',
      capabilities: {},
      skills: [],
      interfaces: [],
      // The server-side AgentCard requires camelCase createdAt /
      // updatedAt on every PUT. The handler stamps updatedAt fresh,
      // but createdAt has to come from the body.
      createdAt: now,
      updatedAt: now,
    }
  }, [initial, defaults])

  const [text, setText] = useState('')
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!open) return
    // Defer to avoid the react-hooks/set-state-in-effect lint
    // (cascading renders); the editor reflows on the next tick.
    const t = setTimeout(() => {
      setText(JSON.stringify(seed, null, 2))
      setError(null)
    }, 0)
    return () => clearTimeout(t)
  }, [open, seed])

  function submit() {
    try {
      const parsed = JSON.parse(text) as AgentCard
      // Defensive: snap the identity fields back to the path-scoped
      // values so the server-side check_card_identity guard always
      // passes regardless of what the operator typed.
      parsed.agentId = defaults.agentId
      parsed.namespace = defaults.namespace
      parsed.tenant = defaults.tenant
      setError(null)
      onSubmit(parsed)
    } catch (e) {
      setError(`Invalid JSON: ${(e as Error).message}`)
    }
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={initial ? 'Edit AgentCard' : 'Create AgentCard'}
      size="lg"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button onClick={submit} loading={loading} disabled={loading}>
            {initial ? 'Save' : 'Create'}
          </Button>
        </>
      }
    >
      <p className={styles.muted}>
        Identity (`agentId` / `namespace` / `tenant`) is enforced from the URL;
        edits to those fields are ignored. Server validates the full A2A schema on save.
      </p>
      <textarea
        className={shared.textarea}
        rows={20}
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      {error && <p className={styles.adminWarning}>{error}</p>}
    </Modal>
  )
}

// ---- Admin state modal (Suspend / Ban) ----

function AdminStateModal({
  open,
  mode,
  onClose,
  onSubmit,
  loading,
}: {
  open: boolean
  mode: 'suspend' | 'ban' | null
  onClose: () => void
  onSubmit: (body: SetBusAgentAdminState) => void
  loading: boolean
}) {
  const [reason, setReason] = useState('')
  const [expiresAt, setExpiresAt] = useState('')

  useEffect(() => {
    if (!open) return
    const t = setTimeout(() => {
      setReason('')
      setExpiresAt('')
    }, 0)
    return () => clearTimeout(t)
  }, [open])

  if (!mode) return null

  const isSuspend = mode === 'suspend'

  function submit() {
    const body: SetBusAgentAdminState = {
      admin_state: isSuspend ? 'suspended' : 'banned',
      reason: reason.trim() || undefined,
    }
    if (isSuspend && expiresAt) {
      // datetime-local gives `YYYY-MM-DDTHH:MM` — append :00Z for RFC-3339.
      body.expires_at = `${expiresAt}:00Z`
    }
    onSubmit(body)
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={isSuspend ? 'Suspend agent' : 'Ban agent'}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button
            variant={isSuspend ? 'secondary' : 'danger'}
            onClick={submit}
            loading={loading}
          >
            {isSuspend ? 'Suspend' : 'Ban'}
          </Button>
        </>
      }
    >
      <div className={styles.formStack}>
        <p className={styles.muted}>
          {isSuspend
            ? 'Suspended agents stop routing. Reinstate manually or set an auto-expire below.'
            : 'Banned agents stop routing permanently. The row is kept so the audit metadata survives.'}
        </p>
        <label>
          <span>Reason (surfaced to the caller on a blocked send)</span>
          <textarea
            className={shared.textarea}
            rows={3}
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            placeholder="investigating runaway tool calls"
          />
        </label>
        {isSuspend && (
          <label>
            <span>Auto-reinstate at (optional)</span>
            <Input
              type="datetime-local"
              value={expiresAt}
              onChange={(e) => setExpiresAt(e.target.value)}
            />
          </label>
        )}
      </div>
    </Modal>
  )
}
