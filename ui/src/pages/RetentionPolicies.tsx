import { useState } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import {
  Plus, Pencil, Trash2, Clock,
} from 'lucide-react'
import {
  useRetentionPolicies,
  useRetentionPolicy,
  useCreateRetention,
  useUpdateRetention,
  useDeleteRetention,
} from '../api/hooks/useRetention'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Modal } from '../components/ui/Modal'
import { DeleteConfirmModal } from '../components/ui/DeleteConfirmModal'
import { Drawer } from '../components/ui/Drawer'
import { Tabs } from '../components/ui/Tabs'
import { useToast } from '../components/ui/useToast'
import { relativeTime, formatDurationSeconds, parseLabels, labelsToText } from '../lib/format'
import type {
  RetentionPolicy,
  CreateRetentionRequest,
  UpdateRetentionRequest,
} from '../types'
import shared from '../styles/shared.module.css'
import styles from './RetentionPolicies.module.css'

// ---- Helpers ----

// ---- Column definition ----

const col = createColumnHelper<RetentionPolicy>()

// ---- Component ----

export function RetentionPolicies() {
  const { toast } = useToast()

  // Filter state
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')

  // Create modal
  const [showCreate, setShowCreate] = useState(false)

  // Edit modal
  const [editTarget, setEditTarget] = useState<RetentionPolicy | null>(null)

  // Detail drawer
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detailTab, setDetailTab] = useState('overview')

  // Delete confirmation
  const [deleteTarget, setDeleteTarget] = useState<RetentionPolicy | null>(null)

  // Data
  const { data, isLoading } = useRetentionPolicies({
    namespace: ns || undefined,
    tenant: tenant || undefined,
  })

  const { data: selectedPolicy } = useRetentionPolicy(selectedId ?? undefined)

  // Mutations
  const createMutation = useCreateRetention()
  const updateMutation = useUpdateRetention()
  const deleteMutation = useDeleteRetention()

  const handleDelete = () => {
    if (!deleteTarget) return
    deleteMutation.mutate(deleteTarget.id, {
      onSuccess: () => {
        toast('success', 'Retention policy deleted')
        setDeleteTarget(null)
        if (selectedId === deleteTarget.id) setSelectedId(null)
      },
      onError: (e) => toast('error', 'Delete failed', (e as Error).message),
    })
  }

  const columns = [
    col.accessor('tenant', {
      header: 'Tenant',
      cell: (info) => <span className={shared.detailValue}>{info.getValue()}</span>,
    }),
    col.accessor('namespace', {
      header: 'Namespace',
      cell: (info) => <span className={shared.detailValue}>{info.getValue()}</span>,
    }),
    col.accessor('audit_ttl_seconds', {
      header: 'Audit TTL',
      cell: (info) => <span className={styles.ttlValue}>{formatDurationSeconds(info.getValue())}</span>,
    }),
    col.accessor('state_ttl_seconds', {
      header: 'State TTL',
      cell: (info) => <span className={styles.ttlValue}>{formatDurationSeconds(info.getValue())}</span>,
    }),
    col.accessor('event_ttl_seconds', {
      header: 'Event TTL',
      cell: (info) => <span className={styles.ttlValue}>{formatDurationSeconds(info.getValue())}</span>,
    }),
    col.accessor('compliance_hold', {
      header: 'Compliance Hold',
      cell: (info) => (
        <Badge variant={info.getValue() ? 'warning' : 'neutral'}>
          {info.getValue() ? 'Enabled' : 'Disabled'}
        </Badge>
      ),
    }),
    col.accessor('enabled', {
      header: 'Status',
      cell: (info) => (
        <Badge variant={info.getValue() ? 'success' : 'warning'}>
          {info.getValue() ? 'Enabled' : 'Disabled'}
        </Badge>
      ),
    }),
    col.display({
      id: 'actions',
      header: 'Actions',
      cell: (info) => {
        const row = info.row.original
        return (
          <div
            className={shared.actionsCell}
            onClick={(e) => e.stopPropagation()}
            role="group"
            aria-label="Row actions"
          >
            <Button
              variant="ghost"
              size="sm"
              icon={<Pencil className="h-3.5 w-3.5" />}
              onClick={() => setEditTarget(row)}
              aria-label="Edit"
            >
              Edit
            </Button>
            <Button
              variant="ghost"
              size="sm"
              icon={<Trash2 className="h-3.5 w-3.5" />}
              onClick={() => setDeleteTarget(row)}
              aria-label="Delete"
            >
              Delete
            </Button>
          </div>
        )
      },
    }),
  ]

  return (
    <div>
      <PageHeader
        title="Data Retention Policies"
        subtitle="Configure TTL and retention rules for audit logs, state, and events"
        actions={
          <Button
            icon={<Plus className="h-3.5 w-3.5" />}
            onClick={() => setShowCreate(true)}
          >
            Create Policy
          </Button>
        }
      />

      <div className={shared.filterBar}>
        <Input
          placeholder="Namespace"
          value={ns}
          onChange={(e) => setNs(e.target.value)}
        />
        <Input
          placeholder="Tenant"
          value={tenant}
          onChange={(e) => setTenant(e.target.value)}
        />
      </div>

      <DataTable
        data={data?.policies ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={(row) => {
          setSelectedId(row.id)
          setDetailTab('overview')
        }}
        emptyTitle="No retention policies"
        emptyDescription="Create a retention policy to configure TTL for audit logs, state, and events."
      />

      {/* Create modal */}
      <RetentionFormModal
        open={showCreate}
        onClose={() => setShowCreate(false)}
        onSubmit={(req) => {
          createMutation.mutate(req, {
            onSuccess: (res) => {
              toast('success', 'Retention policy created', `ID: ${res.id}`)
              setShowCreate(false)
            },
            onError: (e) => toast('error', 'Create failed', (e as Error).message),
          })
        }}
        loading={createMutation.isPending}
        title="Create Retention Policy"
      />

      {/* Edit modal */}
      <RetentionFormModal
        open={!!editTarget}
        onClose={() => setEditTarget(null)}
        onSubmit={(req) => {
          if (!editTarget) return
          const body: UpdateRetentionRequest = {
            audit_ttl_seconds: req.audit_ttl_seconds,
            state_ttl_seconds: req.state_ttl_seconds,
            event_ttl_seconds: req.event_ttl_seconds,
            compliance_hold: req.compliance_hold,
            enabled: req.enabled,
            description: req.description,
            labels: req.labels,
          }
          updateMutation.mutate({ id: editTarget.id, body }, {
            onSuccess: () => {
              toast('success', 'Retention policy updated')
              setEditTarget(null)
            },
            onError: (e) => toast('error', 'Update failed', (e as Error).message),
          })
        }}
        loading={updateMutation.isPending}
        title="Edit Retention Policy"
        initial={editTarget ?? undefined}
      />

      {/* Detail drawer */}
      <Drawer
        open={!!selectedId}
        onClose={() => setSelectedId(null)}
        title={selectedPolicy?.description || `Retention Policy ${selectedId?.slice(0, 12) ?? ''}`}
        wide
      >
        {selectedPolicy && (
          <RetentionDetailView
            policy={selectedPolicy}
            tab={detailTab}
            onTabChange={setDetailTab}
            onEdit={() => {
              setEditTarget(selectedPolicy)
            }}
            onDelete={() => {
              setDeleteTarget(selectedPolicy)
            }}
          />
        )}
      </Drawer>

      {/* Delete confirmation modal */}
      <DeleteConfirmModal
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={handleDelete}
        loading={deleteMutation.isPending}
        title="Delete Retention Policy"
        name={deleteTarget ? `${deleteTarget.tenant}/${deleteTarget.namespace}` : ''}
      />
    </div>
  )
}

// ---- Create/Edit Form Modal ----

function RetentionFormModal({ open, onClose, onSubmit, loading, title, initial }: {
  open: boolean
  onClose: () => void
  onSubmit: (req: CreateRetentionRequest) => void
  loading: boolean
  title: string
  initial?: RetentionPolicy
}) {
  const [formNs, setFormNs] = useState(initial?.namespace ?? '')
  const [formTenant, setFormTenant] = useState(initial?.tenant ?? '')
  const [auditTtl, setAuditTtl] = useState(initial?.audit_ttl_seconds?.toString() ?? '')
  const [stateTtl, setStateTtl] = useState(initial?.state_ttl_seconds?.toString() ?? '')
  const [eventTtl, setEventTtl] = useState(initial?.event_ttl_seconds?.toString() ?? '')
  const [complianceHold, setComplianceHold] = useState(initial?.compliance_hold ?? false)
  const [enabled, setEnabled] = useState(initial?.enabled ?? true)
  const [description, setDescription] = useState(initial?.description ?? '')
  const [labelsText, setLabelsText] = useState(
    initial?.labels ? labelsToText(initial.labels) : '',
  )

  // Reset form when initial changes (opening edit modal for different item)
  const initialId = initial?.id

  const handleSubmit = () => {
    const labels = parseLabels(labelsText)
    onSubmit({
      namespace: formNs,
      tenant: formTenant,
      audit_ttl_seconds: auditTtl ? parseInt(auditTtl, 10) : null,
      state_ttl_seconds: stateTtl ? parseInt(stateTtl, 10) : null,
      event_ttl_seconds: eventTtl ? parseInt(eventTtl, 10) : null,
      compliance_hold: complianceHold,
      enabled,
      description: description || null,
      labels: Object.keys(labels).length > 0 ? labels : undefined,
    })
  }

  const isEdit = !!initial
  const isValid = formNs && formTenant

  return (
    <Modal
      key={initialId ?? 'create'}
      open={open}
      onClose={onClose}
      title={title}
      size="lg"
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>Cancel</Button>
          <Button
            loading={loading}
            onClick={handleSubmit}
            disabled={!isValid}
            icon={<Clock className="h-3.5 w-3.5" />}
          >
            {isEdit ? 'Update' : 'Create'}
          </Button>
        </>
      }
    >
      <div className={shared.formSection}>
        <div className={shared.formGrid}>
          <Input
            label="Namespace *"
            value={formNs}
            onChange={(e) => setFormNs(e.target.value)}
            placeholder="prod"
            disabled={isEdit}
          />
          <Input
            label="Tenant *"
            value={formTenant}
            onChange={(e) => setFormTenant(e.target.value)}
            placeholder="acme"
            disabled={isEdit}
          />
        </div>

        <div className={shared.formGrid}>
          <Input
            label="Audit TTL (seconds)"
            type="number"
            value={auditTtl}
            onChange={(e) => setAuditTtl(e.target.value)}
            placeholder="2592000 (30 days)"
            min="0"
          />
          <Input
            label="State TTL (seconds)"
            type="number"
            value={stateTtl}
            onChange={(e) => setStateTtl(e.target.value)}
            placeholder="604800 (7 days)"
            min="0"
          />
        </div>

        <div className={shared.formGrid}>
          <Input
            label="Event TTL (seconds)"
            type="number"
            value={eventTtl}
            onChange={(e) => setEventTtl(e.target.value)}
            placeholder="86400 (1 day)"
            min="0"
          />
          <Select
            label="Compliance Hold"
            options={[
              { value: 'false', label: 'Disabled' },
              { value: 'true', label: 'Enabled' },
            ]}
            value={complianceHold ? 'true' : 'false'}
            onChange={(e) => setComplianceHold(e.target.value === 'true')}
          />
        </div>

        <Select
          label="Status"
          options={[
            { value: 'true', label: 'Enabled' },
            { value: 'false', label: 'Disabled' },
          ]}
          value={enabled ? 'true' : 'false'}
          onChange={(e) => setEnabled(e.target.value === 'true')}
        />

        <Input
          label="Description"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="30-day retention for production audit logs"
        />

        <div>
          <label className={shared.textareaLabel} htmlFor="retention-labels">Labels (key=value, one per line)</label>
          <textarea
            id="retention-labels"
            value={labelsText}
            onChange={(e) => setLabelsText(e.target.value)}
            className={styles.textarea}
            placeholder={"team=platform\ntier=enterprise"}
          />
        </div>
      </div>
    </Modal>
  )
}

// ---- Detail View ----

function RetentionDetailView({ policy, tab, onTabChange, onEdit, onDelete }: {
  policy: RetentionPolicy
  tab: string
  onTabChange: (t: string) => void
  onEdit: () => void
  onDelete: () => void
}) {
  return (
    <div>
      <div className={styles.tabContainer}>
        <Tabs
          tabs={[
            { id: 'overview', label: 'Overview' },
          ]}
          active={tab}
          onChange={onTabChange}
          size="sm"
        />
      </div>

      {tab === 'overview' && (
        <div className={styles.detailContent}>
          {Object.entries({
            'ID': policy.id,
            'Namespace': policy.namespace,
            'Tenant': policy.tenant,
            'Audit TTL': formatDurationSeconds(policy.audit_ttl_seconds),
            'State TTL': formatDurationSeconds(policy.state_ttl_seconds),
            'Event TTL': formatDurationSeconds(policy.event_ttl_seconds),
            'Compliance Hold': policy.compliance_hold ? 'Enabled' : 'Disabled',
            'Status': policy.enabled ? 'Enabled' : 'Disabled',
            'Description': policy.description ?? '-',
            'Created': relativeTime(policy.created_at),
            'Updated': relativeTime(policy.updated_at),
          }).map(([k, v]) => (
            <div key={k} className={shared.detailRow}>
              <span className={shared.detailLabel}>{k}</span>
              <span className={styles.detailValueWrap}>{v}</span>
            </div>
          ))}

          {policy.labels && Object.keys(policy.labels).length > 0 && (
            <div>
              <h3 className={styles.sectionTitle}>Labels</h3>
              {Object.entries(policy.labels).map(([k, v]) => (
                <div key={k} className={shared.detailRow}>
                  <span className={shared.detailLabel}>{k}</span>
                  <span className={shared.detailValue}>{v}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      <div className={shared.actionButtons}>
        <Button
          variant="secondary"
          size="sm"
          icon={<Pencil className="h-3.5 w-3.5" />}
          onClick={onEdit}
        >
          Edit
        </Button>
        <Button
          variant="danger"
          size="sm"
          icon={<Trash2 className="h-3.5 w-3.5" />}
          onClick={onDelete}
        >
          Delete
        </Button>
      </div>
    </div>
  )
}
