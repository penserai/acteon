// Top-level Agents control plane.
//
// List view with namespace/tenant/admin_state/capability filters,
// a Register Agent dialog, and per-row links into the detail page
// (/agents/{ns}/{t}/{id}). Lifecycle actions live on the detail page
// — keeping the list focused on triage ("which agents need
// attention") rather than letting destructive ops happen in a single
// click from a table row.

import { useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { createColumnHelper } from '@tanstack/react-table'
import { Plus } from 'lucide-react'

import {
  useBusAgents,
  useRegisterBusAgent,
  type BusAgent,
  type RegisterBusAgentReq,
} from '../api/hooks/useBus'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Modal } from '../components/ui/Modal'
import { useToast } from '../components/ui/useToast'
import { relativeTime, parseLabels } from '../lib/format'
import shared from '../styles/shared.module.css'
import styles from './Agents.module.css'

const ADMIN_OPTIONS = [
  { value: '', label: 'All admin states' },
  { value: 'active', label: 'Active' },
  { value: 'suspended', label: 'Suspended' },
  { value: 'banned', label: 'Banned' },
]

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

const ch = createColumnHelper<BusAgent>()

export function Agents() {
  const navigate = useNavigate()
  const [namespace, setNamespace] = useState('')
  const [tenant, setTenant] = useState('')
  const [adminState, setAdminState] = useState('')
  const [capabilityFilter, setCapabilityFilter] = useState('')
  const [registerOpen, setRegisterOpen] = useState(false)

  const { data: rawAgents, isLoading } = useBusAgents({
    namespace: namespace || undefined,
    tenant: tenant || undefined,
    admin_state: adminState || undefined,
  })

  const agents = useMemo(() => {
    if (!rawAgents) return []
    if (!capabilityFilter) return rawAgents
    const needle = capabilityFilter.toLowerCase()
    return rawAgents.filter((a) =>
      a.capabilities.some((c) => c.toLowerCase().includes(needle)),
    )
  }, [rawAgents, capabilityFilter])

  const columns = useMemo(
    () => [
      ch.accessor('agent_id', {
        header: 'Agent',
        cell: (info) => <span className={styles.idCell}>{info.getValue()}</span>,
      }),
      ch.accessor((a) => `${a.namespace}/${a.tenant}`, {
        id: 'scope',
        header: 'Scope',
        cell: (info) => <span className={styles.idCell}>{info.getValue()}</span>,
      }),
      ch.accessor('capabilities', {
        header: 'Capabilities',
        cell: (info) => (
          <div className={styles.badgeRow}>
            {info.getValue().slice(0, 4).map((c) => (
              <Badge key={c} variant="info" size="sm">
                {c}
              </Badge>
            ))}
            {info.getValue().length > 4 && (
              <span className={styles.muted}>+{info.getValue().length - 4}</span>
            )}
          </div>
        ),
      }),
      ch.accessor('status', {
        header: 'Liveness',
        cell: (info) => (
          <Badge variant={statusVariant(info.getValue())} size="sm">
            {info.getValue()}
          </Badge>
        ),
      }),
      ch.accessor('admin_state', {
        header: 'Admin',
        cell: (info) => (
          <Badge variant={adminVariant(info.getValue())} size="sm">
            {info.getValue() ?? 'active'}
          </Badge>
        ),
      }),
      ch.accessor('last_heartbeat_at', {
        header: 'Heartbeat',
        cell: (info) => {
          const v = info.getValue()
          return <span className={styles.muted}>{v ? relativeTime(v) : 'never'}</span>
        },
      }),
    ],
    [],
  )

  return (
    <div>
      <PageHeader
        title="Agents"
        subtitle="Bus-resident agent identities — registry, liveness, and operator lifecycle."
        actions={
          <Button icon={<Plus className="h-4 w-4" />} onClick={() => setRegisterOpen(true)}>
            Register Agent
          </Button>
        }
      />

      <div className={shared.filterBar}>
        <Input
          placeholder="Namespace"
          value={namespace}
          onChange={(e) => setNamespace(e.target.value)}
        />
        <Input
          placeholder="Tenant"
          value={tenant}
          onChange={(e) => setTenant(e.target.value)}
        />
        <Select
          options={ADMIN_OPTIONS}
          value={adminState}
          onChange={(e) => setAdminState(e.target.value)}
        />
        <Input
          placeholder="Capability contains…"
          value={capabilityFilter}
          onChange={(e) => setCapabilityFilter(e.target.value)}
        />
      </div>

      <DataTable
        columns={columns}
        data={agents}
        loading={isLoading}
        emptyTitle="No agents"
        emptyDescription="No agents match the current filters. Register one with the button above, or relax the scope filters."
        onRowClick={(row) =>
          navigate(
            `/agents/${encodeURIComponent(row.namespace)}/${encodeURIComponent(row.tenant)}/${encodeURIComponent(row.agent_id)}`,
          )
        }
      />

      <RegisterAgentModal
        open={registerOpen}
        onClose={() => setRegisterOpen(false)}
        defaultNamespace={namespace}
        defaultTenant={tenant}
        onRegistered={(a) =>
          navigate(
            `/agents/${encodeURIComponent(a.namespace)}/${encodeURIComponent(a.tenant)}/${encodeURIComponent(a.agent_id)}`,
          )
        }
      />
    </div>
  )
}

// ---- Register Agent modal ----

function RegisterAgentModal({
  open,
  onClose,
  defaultNamespace,
  defaultTenant,
  onRegistered,
}: {
  open: boolean
  onClose: () => void
  defaultNamespace: string
  defaultTenant: string
  onRegistered: (a: BusAgent) => void
}) {
  const { toast } = useToast()
  const register = useRegisterBusAgent()
  const [agentId, setAgentId] = useState('')
  const [namespace, setNamespace] = useState(defaultNamespace)
  const [tenant, setTenant] = useState(defaultTenant)
  const [displayName, setDisplayName] = useState('')
  const [capabilities, setCapabilities] = useState('')
  const [inboxTopic, setInboxTopic] = useState('')
  const [labelsText, setLabelsText] = useState('')

  function reset() {
    setAgentId('')
    setNamespace(defaultNamespace)
    setTenant(defaultTenant)
    setDisplayName('')
    setCapabilities('')
    setInboxTopic('')
    setLabelsText('')
  }

  function submit() {
    const req: RegisterBusAgentReq = {
      agent_id: agentId.trim(),
      namespace: namespace.trim(),
      tenant: tenant.trim(),
      display_name: displayName.trim() || undefined,
      capabilities: capabilities
        .split(',')
        .map((c) => c.trim())
        .filter(Boolean),
      inbox_topic: inboxTopic.trim() || undefined,
      labels: labelsText ? parseLabels(labelsText) : undefined,
    }
    register.mutate(req, {
      onSuccess: (a) => {
        toast('success', `Registered ${a.agent_id}`)
        reset()
        onClose()
        onRegistered(a)
      },
      onError: (e) => toast('error', 'Register failed', (e as Error).message),
    })
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Register Agent"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            onClick={submit}
            disabled={!agentId || !namespace || !tenant || register.isPending}
          >
            {register.isPending ? 'Registering…' : 'Register'}
          </Button>
        </>
      }
    >
      <div className={styles.formStack}>
        <label>
          <span>Agent ID</span>
          <Input value={agentId} onChange={(e) => setAgentId(e.target.value)} placeholder="planner-01" />
        </label>
        <div className={styles.formRow}>
          <label>
            <span>Namespace</span>
            <Input value={namespace} onChange={(e) => setNamespace(e.target.value)} placeholder="demo" />
          </label>
          <label>
            <span>Tenant</span>
            <Input value={tenant} onChange={(e) => setTenant(e.target.value)} placeholder="acme" />
          </label>
        </div>
        <label>
          <span>Display name</span>
          <Input
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            placeholder="Planning Agent"
          />
        </label>
        <label>
          <span>Capabilities (comma-separated)</span>
          <Input
            value={capabilities}
            onChange={(e) => setCapabilities(e.target.value)}
            placeholder="plan, reason, summarize"
          />
        </label>
        <label>
          <span>Inbox topic (optional override)</span>
          <Input
            value={inboxTopic}
            onChange={(e) => setInboxTopic(e.target.value)}
            placeholder="defaults to {ns}.{tenant}.agents-inbox"
          />
        </label>
        <label>
          <span>Labels (one per line, key=value)</span>
          <textarea
            className={shared.textarea}
            rows={3}
            value={labelsText}
            onChange={(e) => setLabelsText(e.target.value)}
            placeholder="team=platform&#10;cost-center=ai"
          />
        </label>
      </div>
    </Modal>
  )
}
