import { useState } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import {
  Plus, Pause, Play, Trash2, RefreshCw,
} from 'lucide-react'
import {
  useRecurringActions,
  useRecurringAction,
  useCreateRecurring,
  useDeleteRecurring,
  usePauseRecurring,
  useResumeRecurring,
} from '../api/hooks/useRecurring'
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
import { JsonViewer } from '../components/ui/JsonViewer'
import { useToast } from '../components/ui/useToast'
import { relativeTime } from '../lib/format'
import type { RecurringAction, RecurringActionSummary, CreateRecurringActionRequest } from '../types'
import shared from '../styles/shared.module.css'
import styles from './RecurringActions.module.css'

// ---- Cron description helper ----

const CRON_PRESETS = [
  { value: '', label: 'Custom' },
  { value: '0 * * * *', label: 'Every hour' },
  { value: '0 9 * * *', label: 'Daily at 9:00 AM' },
  { value: '0 9 * * 1', label: 'Weekly on Monday at 9:00 AM' },
  { value: '0 9 1 * *', label: 'Monthly on the 1st at 9:00 AM' },
  { value: '*/5 * * * *', label: 'Every 5 minutes' },
  { value: '0 9 * * 1-5', label: 'Weekdays at 9:00 AM' },
  { value: '0 9-17 * * 1-5', label: 'Business hours (9-17, weekdays)' },
]

function describeCron(expr: string): string {
  const parts = expr.trim().split(/\s+/)
  if (parts.length < 5) return ''

  const [minute, hour, dom, month, dow] = parts

  const preset = CRON_PRESETS.find((p) => p.value === expr)
  if (preset && preset.value) return preset.label

  const isEvery = (v: string) => v === '*'
  const isStep = (v: string) => v.startsWith('*/')

  if (isStep(minute) && isEvery(hour) && isEvery(dom) && isEvery(month) && isEvery(dow)) {
    return `Every ${minute.slice(2)} minutes`
  }
  if (!isEvery(minute) && isEvery(hour) && isEvery(dom) && isEvery(month) && isEvery(dow)) {
    return `Every hour at minute ${minute}`
  }
  if (!isEvery(minute) && !isEvery(hour) && isEvery(dom) && isEvery(month) && isEvery(dow)) {
    return `Every day at ${hour}:${minute.padStart(2, '0')}`
  }
  if (!isEvery(minute) && !isEvery(hour) && isEvery(dom) && isEvery(month) && dow === '1') {
    return `Every Monday at ${hour}:${minute.padStart(2, '0')}`
  }
  if (!isEvery(minute) && !isEvery(hour) && isEvery(dom) && isEvery(month) && dow === '1-5') {
    return `Weekdays at ${hour}:${minute.padStart(2, '0')}`
  }
  if (!isEvery(minute) && !isEvery(hour) && dom === '1' && isEvery(month) && isEvery(dow)) {
    return `Monthly on the 1st at ${hour}:${minute.padStart(2, '0')}`
  }

  return expr
}

function statusVariant(enabled: boolean, nextExecution: string | null): 'success' | 'warning' | 'neutral' {
  if (!enabled) return 'warning'
  if (!nextExecution) return 'neutral'
  return 'success'
}

function statusLabel(enabled: boolean, nextExecution: string | null): string {
  if (!enabled) return 'Paused'
  if (!nextExecution) return 'Completed'
  return 'Active'
}

// ---- Column definition ----

const col = createColumnHelper<RecurringActionSummary>()

// ---- Common timezones ----

const TIMEZONE_OPTIONS = [
  { value: 'UTC', label: 'UTC' },
  { value: 'US/Eastern', label: 'US/Eastern' },
  { value: 'US/Central', label: 'US/Central' },
  { value: 'US/Mountain', label: 'US/Mountain' },
  { value: 'US/Pacific', label: 'US/Pacific' },
  { value: 'Europe/London', label: 'Europe/London' },
  { value: 'Europe/Paris', label: 'Europe/Paris' },
  { value: 'Europe/Berlin', label: 'Europe/Berlin' },
  { value: 'Asia/Tokyo', label: 'Asia/Tokyo' },
  { value: 'Asia/Shanghai', label: 'Asia/Shanghai' },
  { value: 'Asia/Kolkata', label: 'Asia/Kolkata' },
  { value: 'Australia/Sydney', label: 'Australia/Sydney' },
]

// ---- Component ----

export function RecurringActions() {
  const { toast } = useToast()

  // Filter state
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')
  const [enabledFilter, setEnabledFilter] = useState('')

  // Create modal
  const [showCreate, setShowCreate] = useState(false)

  // Detail drawer
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detailTab, setDetailTab] = useState('overview')

  // Delete confirmation
  const [deleteTarget, setDeleteTarget] = useState<RecurringActionSummary | null>(null)

  // Data
  const { data, isLoading } = useRecurringActions({
    namespace: ns || undefined,
    tenant: tenant || undefined,
    enabled: enabledFilter || undefined,
  })

  const { data: selectedAction } = useRecurringAction(
    selectedId ?? undefined,
    { namespace: ns || undefined, tenant: tenant || undefined },
  )

  // Mutations
  const createMutation = useCreateRecurring()
  const deleteMutation = useDeleteRecurring()
  const pauseMutation = usePauseRecurring()
  const resumeMutation = useResumeRecurring()

  const handlePauseResume = (action: RecurringActionSummary) => {
    const mutation = action.enabled ? pauseMutation : resumeMutation
    mutation.mutate(
      { id: action.id, namespace: action.namespace, tenant: action.tenant },
      {
        onSuccess: () => toast('success', action.enabled ? 'Recurring action paused' : 'Recurring action resumed'),
        onError: (e) => toast('error', 'Operation failed', (e as Error).message),
      },
    )
  }

  const handleDelete = () => {
    if (!deleteTarget) return
    deleteMutation.mutate(
      { id: deleteTarget.id, namespace: deleteTarget.namespace, tenant: deleteTarget.tenant },
      {
        onSuccess: () => {
          toast('success', 'Recurring action deleted')
          setDeleteTarget(null)
          if (selectedId === deleteTarget.id) setSelectedId(null)
        },
        onError: (e) => toast('error', 'Delete failed', (e as Error).message),
      },
    )
  }

  const columns = [
    col.accessor('description', {
      header: 'Name',
      cell: (info) => (
        <span className={shared.detailValue}>
          {info.getValue() || info.row.original.id.slice(0, 12) + '...'}
        </span>
      ),
    }),
    col.accessor('cron_expr', {
      header: 'Schedule',
      cell: (info) => (
        <div>
          <span className={styles.cronCell}>{info.getValue()}</span>
          <span className={styles.cronDescription}>
            {describeCron(info.getValue())}
            {info.row.original.timezone !== 'UTC' ? ` (${info.row.original.timezone})` : ''}
          </span>
        </div>
      ),
    }),
    col.display({
      id: 'status',
      header: 'Status',
      cell: (info) => {
        const row = info.row.original
        return (
          <Badge variant={statusVariant(row.enabled, row.next_execution_at)}>
            {statusLabel(row.enabled, row.next_execution_at)}
          </Badge>
        )
      },
    }),
    col.accessor('next_execution_at', {
      header: 'Next Execution',
      cell: (info) => {
        const val = info.getValue()
        return val
          ? <span className={styles.timestampCell} title={val}>{relativeTime(val)}</span>
          : <span className={styles.timestampCell}>-</span>
      },
    }),
    col.accessor('execution_count', {
      header: 'Executions',
      cell: (info) => <span className={styles.countCell}>{info.getValue().toLocaleString()}</span>,
    }),
    col.accessor('provider', {
      header: 'Provider',
      cell: (info) => <span className={shared.detailValue}>{info.getValue()}</span>,
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
              icon={row.enabled ? <Pause className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}
              onClick={() => handlePauseResume(row)}
              aria-label={row.enabled ? 'Pause' : 'Resume'}
            >
              {row.enabled ? 'Pause' : 'Resume'}
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
        title="Recurring Actions"
        subtitle="Cron-scheduled actions that fire on a recurring basis"
        actions={
          <Button
            icon={<Plus className="h-3.5 w-3.5" />}
            onClick={() => setShowCreate(true)}
          >
            Create
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
        <Select
          options={[
            { value: '', label: 'All Statuses' },
            { value: 'true', label: 'Active' },
            { value: 'false', label: 'Paused' },
          ]}
          value={enabledFilter}
          onChange={(e) => setEnabledFilter(e.target.value)}
        />
      </div>

      <DataTable
        data={data?.recurring_actions ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={(row) => {
          setSelectedId(row.id)
          setDetailTab('overview')
        }}
        emptyTitle="No recurring actions"
        emptyDescription="Create a recurring action to schedule cron-based dispatches."
      />

      {/* Create modal */}
      <CreateRecurringModal
        open={showCreate}
        onClose={() => setShowCreate(false)}
        onSubmit={(req) => {
          createMutation.mutate(req, {
            onSuccess: (res) => {
              toast('success', 'Recurring action created', `ID: ${res.id}`)
              setShowCreate(false)
            },
            onError: (e) => toast('error', 'Create failed', (e as Error).message),
          })
        }}
        loading={createMutation.isPending}
      />

      {/* Detail drawer */}
      <Drawer
        open={!!selectedId}
        onClose={() => setSelectedId(null)}
        title={selectedAction?.description || `Recurring ${selectedId?.slice(0, 12) ?? ''}`}
        wide
      >
        {selectedAction && (
          <RecurringDetailView
            action={selectedAction}
            tab={detailTab}
            onTabChange={setDetailTab}
            onPauseResume={() => handlePauseResume(selectedAction)}
            onDelete={() => {
              setDeleteTarget(selectedAction)
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
        title="Delete Recurring Action"
        name={deleteTarget?.description || deleteTarget?.id.slice(0, 12) || ''}
        warning="This will stop all future executions and cannot be undone."
      />
    </div>
  )
}

// ---- Create Modal ----

function CreateRecurringModal({ open, onClose, onSubmit, loading }: {
  open: boolean
  onClose: () => void
  onSubmit: (req: CreateRecurringActionRequest) => void
  loading: boolean
}) {
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')
  const [cronExpr, setCronExpr] = useState('')
  const [timezone, setTimezone] = useState('UTC')
  const [provider, setProvider] = useState('')
  const [actionType, setActionType] = useState('')
  const [payload, setPayload] = useState('{\n  \n}')
  const [payloadError, setPayloadError] = useState('')
  const [description, setDescription] = useState('')
  const [endsAt, setEndsAt] = useState('')
  const [maxExec, setMaxExec] = useState('')

  const cronDesc = cronExpr ? describeCron(cronExpr) : ''
  const isCronValid = cronExpr.trim().split(/\s+/).length >= 5

  const handlePreset = (value: string) => {
    if (value) setCronExpr(value)
  }

  const handleSubmit = () => {
    let parsed: Record<string, unknown>
    try {
      parsed = JSON.parse(payload)
      setPayloadError('')
    } catch {
      setPayloadError('Invalid JSON')
      return
    }

    onSubmit({
      namespace: ns,
      tenant,
      cron_expression: cronExpr,
      timezone,
      provider,
      action_type: actionType,
      payload: parsed,
      description: description || undefined,
      ends_at: endsAt || undefined,
      max_executions: maxExec ? parseInt(maxExec, 10) : undefined,
    })
  }

  const isValid = ns && tenant && cronExpr && isCronValid && provider && actionType

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Create Recurring Action"
      size="lg"
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>Cancel</Button>
          <Button
            loading={loading}
            onClick={handleSubmit}
            disabled={!isValid}
            icon={<RefreshCw className="h-3.5 w-3.5" />}
          >
            Create
          </Button>
        </>
      }
    >
      <div className={shared.formSection}>
        <div className={shared.formGrid}>
          <Input label="Namespace *" value={ns} onChange={(e) => setNs(e.target.value)} placeholder="prod" />
          <Input label="Tenant *" value={tenant} onChange={(e) => setTenant(e.target.value)} placeholder="acme" />
        </div>

        <div className={styles.cronPresetRow}>
          <Input
            label="Cron Expression *"
            value={cronExpr}
            onChange={(e) => setCronExpr(e.target.value)}
            placeholder="0 9 * * MON-FRI"
          />
          <div className={styles.cronPresetSelect}>
            <Select
              label="Presets"
              options={CRON_PRESETS}
              value=""
              onChange={(e) => handlePreset(e.target.value)}
            />
          </div>
        </div>
        {cronExpr && (
          <p className={isCronValid ? styles.cronHint : styles.cronHintError}>
            {isCronValid ? cronDesc || cronExpr : 'Invalid cron expression (need 5 fields: minute hour day month weekday)'}
          </p>
        )}

        <Select
          label="Timezone"
          options={TIMEZONE_OPTIONS}
          value={timezone}
          onChange={(e) => setTimezone(e.target.value)}
        />

        <div className={shared.formGrid}>
          <Input label="Provider *" value={provider} onChange={(e) => setProvider(e.target.value)} placeholder="email" />
          <Input label="Action Type *" value={actionType} onChange={(e) => setActionType(e.target.value)} placeholder="send-notification" />
        </div>

        <div>
          <label className={shared.textareaLabel} htmlFor="recurring-payload">Payload (JSON) *</label>
          <textarea
            id="recurring-payload"
            value={payload}
            onChange={(e) => setPayload(e.target.value)}
            className={styles.textarea}
          />
          {payloadError && <p className={styles.errorText}>{payloadError}</p>}
        </div>

        <Input
          label="Description"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="Weekday morning digest"
        />

        <div className={styles.optionalSection}>
          <span className={styles.optionalLabel}>Optional Settings</span>
          <div className={shared.formGrid}>
            <Input
              label="End Date"
              type="datetime-local"
              value={endsAt}
              onChange={(e) => setEndsAt(e.target.value)}
            />
            <Input
              label="Max Executions"
              type="number"
              value={maxExec}
              onChange={(e) => setMaxExec(e.target.value)}
              placeholder="Unlimited"
              min="1"
            />
          </div>
        </div>
      </div>
    </Modal>
  )
}

// ---- Detail View ----

function RecurringDetailView({ action, tab, onTabChange, onPauseResume, onDelete }: {
  action: RecurringAction
  tab: string
  onTabChange: (t: string) => void
  onPauseResume: () => void
  onDelete: () => void
}) {
  return (
    <div>
      <div className={styles.tabContainer}>
        <Tabs
          tabs={[
            { id: 'overview', label: 'Overview' },
            { id: 'template', label: 'Action Template' },
          ]}
          active={tab}
          onChange={onTabChange}
          size="sm"
        />
      </div>

      {tab === 'overview' && (
        <div className={styles.detailContent}>
          {Object.entries({
            'ID': action.id,
            'Namespace': action.namespace,
            'Tenant': action.tenant,
            'Status': statusLabel(action.enabled, action.next_execution_at),
            'Schedule': `${action.cron_expr} (${describeCron(action.cron_expr)})`,
            'Timezone': action.timezone,
            'Provider': action.provider,
            'Action Type': action.action_type,
            'Next Execution': action.next_execution_at ?? '-',
            'Last Executed': action.last_executed_at ? relativeTime(action.last_executed_at) : 'Never',
            'Execution Count': action.execution_count.toLocaleString(),
            'Max Executions': action.max_executions?.toLocaleString() ?? 'Unlimited',
            'Created': relativeTime(action.created_at),
            'Updated': relativeTime(action.updated_at),
            'End Date': action.ends_at ?? 'None',
            'Description': action.description ?? '-',
          }).map(([k, v]) => (
            <div key={k} className={shared.detailRow}>
              <span className={shared.detailLabel}>{k}</span>
              <span className={styles.detailValueWrap}>{v}</span>
            </div>
          ))}

          {action.labels && Object.keys(action.labels).length > 0 && (
            <div>
              <h3 className={styles.sectionTitle}>Labels</h3>
              {Object.entries(action.labels).map(([k, v]) => (
                <div key={k} className={shared.detailRow}>
                  <span className={shared.detailLabel}>{k}</span>
                  <span className={shared.detailValue}>{v}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {tab === 'template' && (
        <div>
          <h3 className={styles.sectionTitle}>Payload</h3>
          <div className={styles.jsonViewerCard}>
            <JsonViewer data={action.payload} />
          </div>
          {action.metadata && Object.keys(action.metadata).length > 0 && (
            <div className={styles.subsection}>
              <h3 className={styles.sectionTitle}>Metadata</h3>
              <div className={styles.jsonViewerCard}>
                <JsonViewer data={action.metadata} />
              </div>
            </div>
          )}
          {action.dedup_key && (
            <div className={styles.subsection}>
              <h3 className={styles.sectionTitle}>Dedup Key Template</h3>
              <p className={shared.detailValue}>{action.dedup_key}</p>
            </div>
          )}
        </div>
      )}

      <div className={shared.actionButtons}>
        <Button
          variant={action.enabled ? 'secondary' : 'success'}
          size="sm"
          icon={action.enabled ? <Pause className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}
          onClick={onPauseResume}
        >
          {action.enabled ? 'Pause' : 'Resume'}
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
