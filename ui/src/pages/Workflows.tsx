import { useMemo, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { createColumnHelper } from '@tanstack/react-table'
import { XCircle, Radio, History } from 'lucide-react'
import {
  useWorkflowExecutions,
  useCancelWorkflow,
  useSignalWorkflow,
} from '../api/hooks/useWorkflows'
import { useExecutionHistory } from '../api/hooks/useChains'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Drawer } from '../components/ui/Drawer'
import { Modal } from '../components/ui/Modal'
import { JsonViewer } from '../components/ui/JsonViewer'
import { useToast } from '../components/ui/useToast'
import { absoluteTime, relativeTime } from '../lib/format'
import type { WorkflowExecutionSummary } from '../types'
import shared from '../styles/shared.module.css'
import styles from './Workflows.module.css'

const STATUS_OPTIONS = [
  { value: '', label: 'All Statuses' },
  { value: 'running', label: 'Running' },
  { value: 'waiting_timer', label: 'Waiting Timer' },
  { value: 'waiting_signal', label: 'Waiting Signal' },
  { value: 'completed', label: 'Completed' },
  { value: 'failed', label: 'Failed' },
  { value: 'cancelled', label: 'Cancelled' },
]

const ACTIVE_STATUSES = new Set(['running', 'waiting_timer', 'waiting_signal'])

const col = createColumnHelper<WorkflowExecutionSummary>()

export function Workflows() {
  const [searchParams, setSearchParams] = useSearchParams()
  const [ns, setNs] = useState(searchParams.get('namespace') ?? '')
  const [tenant, setTenant] = useState(searchParams.get('tenant') ?? '')
  const [workflow, setWorkflow] = useState(searchParams.get('workflow') ?? '')
  const statusFilter = searchParams.get('status') ?? ''
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [cancelOpen, setCancelOpen] = useState(false)
  const [cancelReason, setCancelReason] = useState('')
  const [signalOpen, setSignalOpen] = useState(false)
  const [signalName, setSignalName] = useState('')
  const [signalPayload, setSignalPayload] = useState('')

  const setParam = (key: string, value: string) => {
    const next = new URLSearchParams(searchParams)
    if (value) next.set(key, value)
    else next.delete(key)
    setSearchParams(next)
  }

  const { data: executions, isLoading } = useWorkflowExecutions({
    namespace: ns || undefined,
    tenant: tenant || undefined,
    workflow: workflow.trim() || undefined,
    status: statusFilter || undefined,
  })

  const selected = useMemo(
    () => executions?.find((e) => e.execution_id === selectedId),
    [executions, selectedId],
  )
  const { data: history } = useExecutionHistory(selected?.execution_id, {
    namespace: ns,
    tenant,
  })
  const cancel = useCancelWorkflow()
  const signal = useSignalWorkflow()
  const { toast } = useToast()

  const columns = [
    col.accessor('execution_id', {
      header: 'Execution ID',
      cell: (info) => <span className={styles.idCell}>{info.getValue().slice(0, 12)}...</span>,
    }),
    col.accessor('workflow', { header: 'Workflow' }),
    col.accessor('queue', { header: 'Queue' }),
    col.accessor('status', { header: 'Status', cell: (info) => <Badge>{info.getValue()}</Badge> }),
    col.accessor('checkpoints', {
      header: 'Checkpoints',
      cell: (info) => <span className={styles.countCell}>{info.getValue().length}</span>,
      enableSorting: false,
    }),
    col.accessor('created_at', {
      header: 'Created',
      cell: (info) => <span className={styles.timestampCell}>{relativeTime(info.getValue())}</span>,
    }),
    col.accessor('updated_at', {
      header: 'Updated',
      cell: (info) => <span className={styles.timestampCell}>{relativeTime(info.getValue())}</span>,
    }),
  ]

  const handleCancel = () => {
    if (!selected) return
    cancel.mutate(
      {
        executionId: selected.execution_id,
        namespace: ns,
        tenant,
        reason: cancelReason.trim() || undefined,
      },
      {
        onSuccess: () => {
          toast('success', 'Workflow cancelled')
          setCancelOpen(false)
          setCancelReason('')
        },
        onError: (e) => toast('error', 'Cancel failed', (e as Error).message),
      },
    )
  }

  const handleSignal = () => {
    if (!selected) return
    let payload: unknown
    if (signalPayload.trim()) {
      try {
        payload = JSON.parse(signalPayload)
      } catch {
        toast('error', 'Invalid JSON payload')
        return
      }
    }
    signal.mutate(
      {
        executionId: selected.execution_id,
        signalName: signalName.trim(),
        namespace: ns,
        tenant,
        payload,
      },
      {
        onSuccess: () => {
          toast('success', `Signal "${signalName.trim()}" delivered`)
          setSignalOpen(false)
          setSignalName('')
          setSignalPayload('')
        },
        onError: (e) => toast('error', 'Signal failed', (e as Error).message),
      },
    )
  }

  const isActive = !!selected && ACTIVE_STATUSES.has(selected.status)

  return (
    <div>
      <PageHeader title="Workflows" />

      <div className={styles.filterBar}>
        <Input
          placeholder="Namespace"
          value={ns}
          onChange={(e) => {
            setNs(e.target.value)
            setParam('namespace', e.target.value)
          }}
        />
        <Input
          placeholder="Tenant"
          value={tenant}
          onChange={(e) => {
            setTenant(e.target.value)
            setParam('tenant', e.target.value)
          }}
        />
        <Select
          options={STATUS_OPTIONS}
          value={statusFilter}
          onChange={(e) => setParam('status', e.target.value)}
        />
        <Input
          placeholder="Workflow name"
          value={workflow}
          onChange={(e) => {
            setWorkflow(e.target.value)
            setParam('workflow', e.target.value)
          }}
        />
      </div>

      <DataTable
        data={executions ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={(row) => setSelectedId(row.execution_id)}
        emptyTitle="No workflow executions"
        emptyDescription="Enter a namespace and tenant to list workflow executions started via POST /v1/workflows/start."
      />

      <Drawer
        open={!!selected}
        onClose={() => setSelectedId(null)}
        title={selected ? `Workflow: ${selected.workflow}` : 'Workflow'}
        wide
      >
        {selected && (
          <div className={styles.detailContent}>
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Execution ID</span>
              <span className={shared.detailValue}>{selected.execution_id}</span>
            </div>
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Status</span>
              <Badge>{selected.status}</Badge>
            </div>
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Queue</span>
              <span className={shared.detailValue}>{selected.queue}</span>
            </div>
            {selected.parent_id && (
              <div className={shared.detailRow}>
                <span className={shared.detailLabel}>Parent</span>
                <span className={shared.detailValue}>{selected.parent_id}</span>
              </div>
            )}
            {selected.children && selected.children.length > 0 && (
              <div className={shared.detailRow}>
                <span className={shared.detailLabel}>Children</span>
                <span className={shared.detailValue}>{selected.children.join(', ')}</span>
              </div>
            )}
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Created</span>
              <span>{absoluteTime(selected.created_at)}</span>
            </div>
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Updated</span>
              <span>{absoluteTime(selected.updated_at)}</span>
            </div>

            {isActive && (
              <div className={styles.detailActions}>
                <Button
                  variant="secondary"
                  size="sm"
                  icon={<Radio className="h-3.5 w-3.5" />}
                  onClick={() => setSignalOpen(true)}
                >
                  Send Signal
                </Button>
                <Button
                  variant="danger"
                  size="sm"
                  icon={<XCircle className="h-3.5 w-3.5" />}
                  onClick={() => setCancelOpen(true)}
                >
                  Cancel
                </Button>
              </div>
            )}

            {selected.error && <div className={styles.errorBox}>{selected.error}</div>}

            <div>
              <h3 className={shared.sectionTitle}>Input</h3>
              <div className={styles.jsonSection}>
                <JsonViewer data={selected.input} collapsed />
              </div>
            </div>

            {selected.result !== undefined && (
              <div>
                <h3 className={shared.sectionTitle}>Result</h3>
                <div className={styles.jsonSection}>
                  <JsonViewer data={selected.result} collapsed />
                </div>
              </div>
            )}

            {selected.awaiting && (
              <div>
                <h3 className={shared.sectionTitle}>Awaiting</h3>
                <div className={styles.jsonSection}>
                  <JsonViewer data={selected.awaiting} collapsed />
                </div>
              </div>
            )}

            {Object.keys(selected.search_attributes).length > 0 && (
              <div>
                <h3 className={shared.sectionTitle}>Search Attributes</h3>
                <div className={styles.jsonSection}>
                  <JsonViewer data={selected.search_attributes} collapsed />
                </div>
              </div>
            )}

            <div>
              <h3 className={shared.sectionTitle}>
                Checkpoints ({selected.checkpoints.length})
              </h3>
              {selected.checkpoints.length === 0 ? (
                <p className="text-sm text-gray-500">No checkpoints recorded yet.</p>
              ) : (
                <ul className={styles.checkpointList}>
                  {selected.checkpoints.map((cp) => (
                    <li key={cp.seq} className={styles.checkpointItem}>
                      <div className={styles.checkpointHeader}>
                        <span className={styles.checkpointSeq}>#{cp.seq}</span>
                        <span className={styles.checkpointName}>{cp.name}</span>
                        <span className={styles.checkpointTime}>{absoluteTime(cp.recorded_at)}</span>
                      </div>
                      {cp.data !== null && cp.data !== undefined && (
                        <div className={styles.jsonSection}>
                          <JsonViewer data={cp.data} collapsed />
                        </div>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </div>

            {history && history.events.length > 0 && (
              <div>
                <h3 className={shared.sectionTitle}>
                  <History className="inline h-4 w-4 mr-1 text-primary-400" />
                  Event History ({history.events.length})
                </h3>
                <ul className={styles.historyList}>
                  {history.events.map((event) => {
                    const detail = [
                      typeof event.checkpoint_name === 'string' && event.checkpoint_name,
                      typeof event.signal_name === 'string' && `signal: ${event.signal_name}`,
                      typeof event.queue === 'string' && `queue: ${event.queue}`,
                      typeof event.fire_at === 'string' && `fires ${absoluteTime(event.fire_at)}`,
                      typeof event.error === 'string' && event.error && `error: ${event.error}`,
                    ]
                      .filter(Boolean)
                      .join(' -- ')
                    return (
                      <li key={event.event_id} className={styles.historyItem}>
                        <span className={styles.historyEventId}>#{event.event_id}</span>
                        <span className={styles.historyTimestamp}>{absoluteTime(event.timestamp)}</span>
                        <Badge variant={event.event_type.includes('failed') || event.event_type.includes('cancelled') ? 'error' : 'info'} size="sm">
                          {event.event_type}
                        </Badge>
                        {detail && <span className={styles.historyDetail}>{detail}</span>}
                      </li>
                    )
                  })}
                </ul>
              </div>
            )}
          </div>
        )}
      </Drawer>

      <Modal
        open={signalOpen}
        onClose={() => setSignalOpen(false)}
        title="Send Signal"
        footer={
          <>
            <Button variant="secondary" onClick={() => setSignalOpen(false)}>Cancel</Button>
            <Button variant="primary" loading={signal.isPending} disabled={!signalName.trim()} onClick={handleSignal}>
              Deliver
            </Button>
          </>
        }
      >
        <div className="space-y-3">
          <p className="text-sm text-gray-400">
            Deliver an external signal to this workflow execution. If it is
            awaiting a matching signal it resumes immediately; otherwise the
            signal is buffered.
          </p>
          <input
            className="w-full rounded border border-gray-700 bg-transparent px-2 py-1.5 text-sm"
            placeholder="Signal name (e.g. approved)"
            value={signalName}
            onChange={(e) => setSignalName(e.target.value)}
          />
          <textarea
            className="w-full rounded border border-gray-700 bg-transparent px-2 py-1.5 text-sm font-mono"
            rows={4}
            placeholder='Optional JSON payload, e.g. {"approver": "renzo"}'
            value={signalPayload}
            onChange={(e) => setSignalPayload(e.target.value)}
          />
        </div>
      </Modal>

      <Modal
        open={cancelOpen}
        onClose={() => setCancelOpen(false)}
        title="Cancel Workflow"
        footer={
          <>
            <Button variant="secondary" onClick={() => setCancelOpen(false)}>Cancel</Button>
            <Button variant="danger" loading={cancel.isPending} onClick={handleCancel}>Confirm Cancel</Button>
          </>
        }
      >
        <p>
          Cancel workflow <strong>{selected?.workflow}</strong> ({selected?.execution_id.slice(0, 12)})?
        </p>
        <p className="text-sm text-gray-500 mt-2 mb-3">
          Pending tasks and timers are discarded; the execution moves to the cancelled state.
        </p>
        <input
          className="w-full rounded border border-gray-700 bg-transparent px-2 py-1.5 text-sm"
          placeholder="Optional reason"
          value={cancelReason}
          onChange={(e) => setCancelReason(e.target.value)}
        />
      </Modal>
    </div>
  )
}
