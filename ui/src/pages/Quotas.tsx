import { useState } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import {
  Plus, Pencil, Trash2, Gauge,
} from 'lucide-react'
import {
  useQuotas,
  useQuota,
  useQuotaUsage,
  useCreateQuota,
  useUpdateQuota,
  useDeleteQuota,
} from '../api/hooks/useQuotas'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Modal } from '../components/ui/Modal'
import { Drawer } from '../components/ui/Drawer'
import { Tabs } from '../components/ui/Tabs'
import { useToast } from '../components/ui/useToast'
import { relativeTime } from '../lib/format'
import type {
  QuotaPolicy,
  QuotaUsage,
  QuotaWindow,
  OverageBehavior,
  CreateQuotaRequest,
  UpdateQuotaRequest,
} from '../types'
import styles from './Quotas.module.css'

// ---- Helpers ----

const WINDOW_OPTIONS: { value: QuotaWindow; label: string }[] = [
  { value: 'hourly', label: 'Hourly' },
  { value: 'daily', label: 'Daily' },
  { value: 'weekly', label: 'Weekly' },
  { value: 'monthly', label: 'Monthly' },
]

const BEHAVIOR_OPTIONS: { value: OverageBehavior; label: string }[] = [
  { value: 'block', label: 'Block' },
  { value: 'warn', label: 'Warn' },
  { value: 'degrade', label: 'Degrade' },
  { value: 'notify', label: 'Notify' },
]

function behaviorVariant(behavior: OverageBehavior): 'error' | 'warning' | 'info' | 'neutral' {
  switch (behavior) {
    case 'block': return 'error'
    case 'warn': return 'warning'
    case 'degrade': return 'info'
    case 'notify': return 'neutral'
  }
}

function capitalize(w: string): string {
  return w.charAt(0).toUpperCase() + w.slice(1)
}

// ---- Progress bar component ----

function UsageBar({ used, limit }: { used: number; limit: number }) {
  const pct = limit > 0 ? Math.min((used / limit) * 100, 100) : 0
  const colorClass = pct > 90
    ? styles.progressRed
    : pct > 75
      ? styles.progressYellow
      : styles.progressGreen

  return (
    <div className={styles.usageCell}>
      <div className={styles.progressBar}>
        <div
          className={`${styles.progressFill} ${colorClass}`}
          style={{ width: `${pct}%` }}
        />
      </div>
      <span className={styles.usageText}>
        {used.toLocaleString()}/{limit.toLocaleString()}
      </span>
    </div>
  )
}

// ---- Column definition ----

interface QuotaRow extends QuotaPolicy {
  _used?: number
}

const col = createColumnHelper<QuotaRow>()

// ---- Component ----

export function Quotas() {
  const { toast } = useToast()

  // Filter state
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')

  // Create modal
  const [showCreate, setShowCreate] = useState(false)

  // Edit modal
  const [editTarget, setEditTarget] = useState<QuotaPolicy | null>(null)

  // Detail drawer
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detailTab, setDetailTab] = useState('overview')

  // Delete confirmation
  const [deleteTarget, setDeleteTarget] = useState<QuotaPolicy | null>(null)

  // Data
  const { data, isLoading } = useQuotas({
    namespace: ns || undefined,
    tenant: tenant || undefined,
  })

  const { data: selectedQuota } = useQuota(selectedId ?? undefined)
  const { data: selectedUsage } = useQuotaUsage(selectedId ?? undefined)

  // Mutations
  const createMutation = useCreateQuota()
  const updateMutation = useUpdateQuota()
  const deleteMutation = useDeleteQuota()

  const handleDelete = () => {
    if (!deleteTarget) return
    deleteMutation.mutate(deleteTarget.id, {
      onSuccess: () => {
        toast('success', 'Quota deleted')
        setDeleteTarget(null)
        if (selectedId === deleteTarget.id) setSelectedId(null)
      },
      onError: (e) => toast('error', 'Delete failed', (e as Error).message),
    })
  }

  const columns = [
    col.accessor('tenant', {
      header: 'Tenant',
      cell: (info) => <span className={styles.detailValue}>{info.getValue()}</span>,
    }),
    col.accessor('namespace', {
      header: 'Namespace',
      cell: (info) => <span className={styles.detailValue}>{info.getValue()}</span>,
    }),
    col.accessor('max_actions', {
      header: 'Limit',
      cell: (info) => <span className={styles.detailValue}>{info.getValue().toLocaleString()}</span>,
    }),
    col.accessor('window', {
      header: 'Window',
      cell: (info) => capitalize(info.getValue()),
    }),
    col.display({
      id: 'usage',
      header: 'Usage',
      cell: (info) => {
        const row = info.row.original
        return <UsageBar used={row._used ?? 0} limit={row.max_actions} />
      },
    }),
    col.accessor('overage_behavior', {
      header: 'Behavior',
      cell: (info) => (
        <Badge variant={behaviorVariant(info.getValue())}>
          {capitalize(info.getValue())}
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
            className={styles.actionsCell}
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
        title="Quotas"
        subtitle="Rate-based quota policies to limit actions per tenant"
        actions={
          <Button
            icon={<Plus className="h-3.5 w-3.5" />}
            onClick={() => setShowCreate(true)}
          >
            Create Quota
          </Button>
        }
      />

      <div className={styles.filterBar}>
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
        data={data?.quotas ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={(row) => {
          setSelectedId(row.id)
          setDetailTab('overview')
        }}
        emptyTitle="No quotas"
        emptyDescription="Create a quota policy to limit actions per tenant."
      />

      {/* Create modal */}
      <QuotaFormModal
        open={showCreate}
        onClose={() => setShowCreate(false)}
        onSubmit={(req) => {
          createMutation.mutate(req, {
            onSuccess: (res) => {
              toast('success', 'Quota created', `ID: ${res.id}`)
              setShowCreate(false)
            },
            onError: (e) => toast('error', 'Create failed', (e as Error).message),
          })
        }}
        loading={createMutation.isPending}
        title="Create Quota"
      />

      {/* Edit modal */}
      <QuotaFormModal
        open={!!editTarget}
        onClose={() => setEditTarget(null)}
        onSubmit={(req) => {
          if (!editTarget) return
          const body: UpdateQuotaRequest = {
            max_actions: req.max_actions,
            window: req.window,
            overage_behavior: req.overage_behavior,
            enabled: req.enabled,
            description: req.description,
            labels: req.labels,
          }
          updateMutation.mutate({ id: editTarget.id, body }, {
            onSuccess: () => {
              toast('success', 'Quota updated')
              setEditTarget(null)
            },
            onError: (e) => toast('error', 'Update failed', (e as Error).message),
          })
        }}
        loading={updateMutation.isPending}
        title="Edit Quota"
        initial={editTarget ?? undefined}
      />

      {/* Detail drawer */}
      <Drawer
        open={!!selectedId}
        onClose={() => setSelectedId(null)}
        title={selectedQuota?.description || `Quota ${selectedId?.slice(0, 12) ?? ''}`}
        wide
      >
        {selectedQuota && (
          <QuotaDetailView
            quota={selectedQuota}
            usage={selectedUsage ?? null}
            tab={detailTab}
            onTabChange={setDetailTab}
            onEdit={() => {
              setEditTarget(selectedQuota)
            }}
            onDelete={() => {
              setDeleteTarget(selectedQuota)
            }}
          />
        )}
      </Drawer>

      {/* Delete confirmation modal */}
      <Modal
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        title="Delete Quota"
        size="sm"
        footer={
          <>
            <Button variant="secondary" onClick={() => setDeleteTarget(null)}>Cancel</Button>
            <Button
              variant="danger"
              loading={deleteMutation.isPending}
              onClick={handleDelete}
            >
              Delete
            </Button>
          </>
        }
      >
        <p className={styles.deleteWarning}>
          Are you sure you want to delete the quota for{' '}
          <span className={styles.deleteName}>
            {deleteTarget?.tenant}/{deleteTarget?.namespace}
          </span>
          ? This cannot be undone.
        </p>
      </Modal>
    </div>
  )
}

// ---- Create/Edit Form Modal ----

function QuotaFormModal({ open, onClose, onSubmit, loading, title, initial }: {
  open: boolean
  onClose: () => void
  onSubmit: (req: CreateQuotaRequest) => void
  loading: boolean
  title: string
  initial?: QuotaPolicy
}) {
  const [formNs, setFormNs] = useState(initial?.namespace ?? '')
  const [formTenant, setFormTenant] = useState(initial?.tenant ?? '')
  const [maxActions, setMaxActions] = useState(initial?.max_actions?.toString() ?? '1000')
  const [window, setWindow] = useState<QuotaWindow>(initial?.window ?? 'daily')
  const [behavior, setBehavior] = useState<OverageBehavior>(initial?.overage_behavior ?? 'block')
  const [enabled, setEnabled] = useState(initial?.enabled ?? true)
  const [description, setDescription] = useState(initial?.description ?? '')
  const [labelsText, setLabelsText] = useState(
    initial?.labels ? Object.entries(initial.labels).map(([k, v]) => `${k}=${v}`).join('\n') : '',
  )

  // Reset form when initial changes (opening edit modal for different item)
  const initialId = initial?.id
  useState(() => {
    if (initial) {
      setFormNs(initial.namespace)
      setFormTenant(initial.tenant)
      setMaxActions(initial.max_actions.toString())
      setWindow(initial.window)
      setBehavior(initial.overage_behavior)
      setEnabled(initial.enabled)
      setDescription(initial.description ?? '')
      setLabelsText(
        Object.entries(initial.labels).map(([k, v]) => `${k}=${v}`).join('\n'),
      )
    }
  })

  const handleSubmit = () => {
    const labels: Record<string, string> = {}
    for (const line of labelsText.split('\n')) {
      const trimmed = line.trim()
      if (!trimmed) continue
      const eqIdx = trimmed.indexOf('=')
      if (eqIdx > 0) {
        labels[trimmed.slice(0, eqIdx).trim()] = trimmed.slice(eqIdx + 1).trim()
      }
    }

    onSubmit({
      namespace: formNs,
      tenant: formTenant,
      max_actions: parseInt(maxActions, 10) || 0,
      window,
      overage_behavior: behavior,
      enabled,
      description: description || undefined,
      labels: Object.keys(labels).length > 0 ? labels : undefined,
    })
  }

  const isEdit = !!initial
  const isValid = formNs && formTenant && maxActions && parseInt(maxActions, 10) > 0

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
            icon={<Gauge className="h-3.5 w-3.5" />}
          >
            {isEdit ? 'Update' : 'Create'}
          </Button>
        </>
      }
    >
      <div className={styles.formSection}>
        <div className={styles.formGrid}>
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

        <div className={styles.formGrid}>
          <Input
            label="Max Actions *"
            type="number"
            value={maxActions}
            onChange={(e) => setMaxActions(e.target.value)}
            placeholder="1000"
            min="1"
          />
          <Select
            label="Window *"
            options={WINDOW_OPTIONS}
            value={window}
            onChange={(e) => setWindow(e.target.value as QuotaWindow)}
          />
        </div>

        <div className={styles.formGrid}>
          <Select
            label="Overage Behavior *"
            options={BEHAVIOR_OPTIONS}
            value={behavior}
            onChange={(e) => setBehavior(e.target.value as OverageBehavior)}
          />
          <Select
            label="Status"
            options={[
              { value: 'true', label: 'Enabled' },
              { value: 'false', label: 'Disabled' },
            ]}
            value={enabled ? 'true' : 'false'}
            onChange={(e) => setEnabled(e.target.value === 'true')}
          />
        </div>

        <Input
          label="Description"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="Daily action limit for production"
        />

        <div>
          <label className={styles.textareaLabel} htmlFor="quota-labels">Labels (key=value, one per line)</label>
          <textarea
            id="quota-labels"
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

function QuotaDetailView({ quota, usage, tab, onTabChange, onEdit, onDelete }: {
  quota: QuotaPolicy
  usage: QuotaUsage | null
  tab: string
  onTabChange: (t: string) => void
  onEdit: () => void
  onDelete: () => void
}) {
  const usedPct = usage && usage.limit > 0
    ? Math.min((usage.used / usage.limit) * 100, 100)
    : 0
  const pctColorClass = usedPct > 90
    ? styles.progressRed
    : usedPct > 75
      ? styles.progressYellow
      : styles.progressGreen

  return (
    <div>
      <div className={styles.tabContainer}>
        <Tabs
          tabs={[
            { id: 'overview', label: 'Overview' },
            { id: 'usage', label: 'Usage' },
          ]}
          active={tab}
          onChange={onTabChange}
          size="sm"
        />
      </div>

      {tab === 'overview' && (
        <div className={styles.detailContent}>
          {Object.entries({
            'ID': quota.id,
            'Namespace': quota.namespace,
            'Tenant': quota.tenant,
            'Max Actions': quota.max_actions.toLocaleString(),
            'Window': capitalize(quota.window),
            'Overage Behavior': capitalize(quota.overage_behavior),
            'Status': quota.enabled ? 'Enabled' : 'Disabled',
            'Description': quota.description ?? '-',
            'Created': relativeTime(quota.created_at),
            'Updated': relativeTime(quota.updated_at),
          }).map(([k, v]) => (
            <div key={k} className={styles.detailRow}>
              <span className={styles.detailLabel}>{k}</span>
              <span className={styles.detailValueWrap}>{v}</span>
            </div>
          ))}

          {quota.labels && Object.keys(quota.labels).length > 0 && (
            <div>
              <h3 className={styles.sectionTitle}>Labels</h3>
              {Object.entries(quota.labels).map(([k, v]) => (
                <div key={k} className={styles.detailRow}>
                  <span className={styles.detailLabel}>{k}</span>
                  <span className={styles.detailValue}>{v}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {tab === 'usage' && (
        <div>
          {usage ? (
            <div className={styles.usageSection}>
              <h3 className={styles.usageSectionTitle}>
                Current {capitalize(usage.window)} Usage
              </h3>
              <div className={styles.usageBarLarge}>
                <div
                  className={`${styles.usageBarFill} ${pctColorClass}`}
                  style={{ width: `${usedPct}%` }}
                />
              </div>
              <div className={styles.usageStats}>
                <span>{usage.used.toLocaleString()} used</span>
                <span>{usage.remaining.toLocaleString()} remaining</span>
                <span>{usage.limit.toLocaleString()} limit</span>
              </div>
              <div className={styles.detailContent} style={{ marginTop: '1rem' }}>
                <div className={styles.detailRow}>
                  <span className={styles.detailLabel}>Resets At</span>
                  <span className={styles.detailValueWrap}>{relativeTime(usage.resets_at)}</span>
                </div>
                <div className={styles.detailRow}>
                  <span className={styles.detailLabel}>Utilization</span>
                  <span className={styles.detailValueWrap}>{usedPct.toFixed(1)}%</span>
                </div>
              </div>
            </div>
          ) : (
            <p className={styles.detailLabel}>No usage data available.</p>
          )}
        </div>
      )}

      <div className={styles.actionButtons}>
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
