import { useMemo, useState } from 'react'
import { Bot, XCircle } from 'lucide-react'
import { createColumnHelper } from '@tanstack/react-table'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Drawer } from '../components/ui/Drawer'
import { EmptyState } from '../components/ui/EmptyState'
import { Skeleton } from '../components/ui/Skeleton'
import { useToast } from '../components/ui/useToast'
import { relativeTime } from '../lib/format'
import {
  useSwarmRuns,
  useSwarmRun,
  useCancelSwarmRun,
  type SwarmRunSnapshot,
} from '../api/hooks/useSwarmRuns'
import shared from '../styles/shared.module.css'
import styles from './SwarmRuns.module.css'

const STATUS_OPTIONS = [
  { value: '', label: 'All statuses' },
  { value: 'accepted', label: 'Accepted' },
  { value: 'running', label: 'Running' },
  { value: 'adversarial', label: 'Adversarial' },
  { value: 'completed', label: 'Completed' },
  { value: 'failed', label: 'Failed' },
  { value: 'cancelled', label: 'Cancelled' },
  { value: 'cancelling', label: 'Cancelling' },
  { value: 'timed_out', label: 'Timed out' },
]

const TERMINAL = new Set(['completed', 'failed', 'cancelled', 'timed_out'])

const columnHelper = createColumnHelper<SwarmRunSnapshot>()

export function SwarmRuns() {
  const [namespace, setNamespace] = useState('')
  const [tenant, setTenant] = useState('')
  const [status, setStatus] = useState('')
  const [selectedId, setSelectedId] = useState<string | null>(null)

  const params = useMemo(
    () => ({
      namespace: namespace || undefined,
      tenant: tenant || undefined,
      status: status || undefined,
      limit: 100,
    }),
    [namespace, tenant, status],
  )

  const { data, isLoading, error } = useSwarmRuns(params)
  const detail = useSwarmRun(selectedId ?? undefined)
  const cancelMutation = useCancelSwarmRun()
  const { toast } = useToast()

  const columns = useMemo(
    () => [
      columnHelper.accessor('objective', {
        header: 'Objective',
        cell: (info) => (
          <span className={shared.mono} title={info.row.original.run_id}>
            {info.getValue()}
          </span>
        ),
      }),
      columnHelper.accessor('status', {
        header: 'Status',
        cell: (info) => <Badge>{info.getValue()}</Badge>,
      }),
      columnHelper.accessor('namespace', { header: 'Namespace' }),
      columnHelper.accessor('tenant', { header: 'Tenant' }),
      columnHelper.accessor('started_at', {
        header: 'Started',
        cell: (info) => relativeTime(info.getValue()),
      }),
      columnHelper.accessor('finished_at', {
        header: 'Finished',
        cell: (info) =>
          info.getValue() ? relativeTime(info.getValue() as string) : '—',
      }),
    ],
    [],
  )

  const handleCancel = (runId: string) => {
    cancelMutation.mutate(runId, {
      onSuccess: () => toast('success', `Cancellation requested for ${runId}`),
      onError: (e) => toast('error', `Cancel failed: ${(e as Error).message}`),
    })
  }

  return (
    <div>
      <PageHeader title="Swarm Runs" />
      <div className={styles.controls}>
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
          value={status}
          onChange={(e) => setStatus(e.target.value)}
          options={STATUS_OPTIONS}
        />
      </div>

      {isLoading ? (
        <Skeleton className="h-64" />
      ) : error ? (
        <EmptyState
          icon={<Bot className="h-12 w-12" />}
          title="Unable to load swarm runs"
          description={(error as Error).message}
        />
      ) : !data || data.runs.length === 0 ? (
        <EmptyState
          icon={<Bot className="h-12 w-12" />}
          title="No swarm runs"
          description="Dispatch an action with provider=swarm to start a run."
        />
      ) : (
        <DataTable
          columns={columns}
          data={data.runs}
          onRowClick={(row) => setSelectedId(row.run_id)}
        />
      )}

      <Drawer
        open={!!selectedId}
        onClose={() => setSelectedId(null)}
        title={detail.data?.objective ?? 'Swarm run'}
      >
        {detail.data && (
          <div className={styles.detailContent}>
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Run ID</span>
              <span className={shared.mono}>{detail.data.run_id}</span>
            </div>
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Plan ID</span>
              <span className={shared.mono}>{detail.data.plan_id}</span>
            </div>
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Status</span>
              <Badge>{detail.data.status}</Badge>
            </div>
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Namespace / Tenant</span>
              <span>
                {detail.data.namespace} / {detail.data.tenant}
              </span>
            </div>
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Started</span>
              <span>{relativeTime(detail.data.started_at)}</span>
            </div>
            {detail.data.finished_at && (
              <div className={shared.detailRow}>
                <span className={shared.detailLabel}>Finished</span>
                <span>{relativeTime(detail.data.finished_at)}</span>
              </div>
            )}

            {detail.data.metrics && (
              <div>
                <h3 className={shared.sectionTitle}>Metrics</h3>
                <div className={styles.metricsGrid}>
                  {Object.entries(detail.data.metrics).map(([k, v]) => (
                    <div key={k} className={styles.metricCell}>
                      <span className={styles.metricLabel}>
                        {k.replace(/_/g, ' ')}
                      </span>
                      <span className={styles.metricValue}>
                        {v === null || v === undefined ? '—' : String(v)}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {detail.data.error && (
              <div className={styles.errorBox}>{detail.data.error}</div>
            )}

            {!TERMINAL.has(detail.data.status) &&
              detail.data.status !== 'cancelling' && (
                <Button
                  variant="danger"
                  onClick={() => handleCancel(detail.data!.run_id)}
                  disabled={cancelMutation.isPending}
                >
                  <XCircle className="h-4 w-4" />
                  Cancel run
                </Button>
              )}
          </div>
        )}
      </Drawer>
    </div>
  )
}
