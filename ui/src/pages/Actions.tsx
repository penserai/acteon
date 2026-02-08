import { useState, useMemo } from 'react'
import { useSearchParams } from 'react-router-dom'
import { createColumnHelper } from '@tanstack/react-table'
import { RotateCcw } from 'lucide-react'
import { useAudit, useReplayAction } from '../api/hooks/useAudit'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Drawer } from '../components/ui/Drawer'
import { Tabs } from '../components/ui/Tabs'
import { JsonViewer } from '../components/ui/JsonViewer'
import { useToast } from '../components/ui/Toast'
import { relativeTime } from '../lib/format'
import type { AuditRecord, AuditQuery } from '../types'
import styles from './Actions.module.css'

const col = createColumnHelper<AuditRecord>()

const outcomeOptions = ['Executed', 'Failed', 'Suppressed', 'Deduplicated', 'Rerouted', 'Throttled', 'PendingApproval', 'ChainStarted', 'CircuitOpen', 'Scheduled', 'DryRun'].map((v) => ({ value: v, label: v }))

export function Actions() {
  const [searchParams, setSearchParams] = useSearchParams()
  const { toast } = useToast()
  const replay = useReplayAction()
  const [selected, setSelected] = useState<AuditRecord | null>(null)
  const [detailTab, setDetailTab] = useState('overview')

  const query: AuditQuery = useMemo(() => ({
    namespace: searchParams.get('namespace') ?? undefined,
    tenant: searchParams.get('tenant') ?? undefined,
    outcome: searchParams.get('outcome') ?? undefined,
    action_type: searchParams.get('action_type') ?? undefined,
    limit: 50,
    offset: Number(searchParams.get('offset') ?? 0),
  }), [searchParams])

  const { data, isLoading } = useAudit(query)

  const setFilter = (key: string, val: string) => {
    const next = new URLSearchParams(searchParams)
    if (val) next.set(key, val)
    else next.delete(key)
    next.delete('offset')
    setSearchParams(next)
  }

  const handleReplay = (actionId: string) => {
    replay.mutate(actionId, {
      onSuccess: (res) => toast('success', 'Action replayed', `New ID: ${res.new_action_id}`),
      onError: (e) => toast('error', 'Replay failed', (e as Error).message),
    })
  }

  const columns = [
    col.accessor('action_id', {
      header: 'Action ID',
      cell: (info) => <span className={styles.actionId}>{info.getValue().slice(0, 12)}...</span>,
    }),
    col.accessor('namespace', { header: 'Namespace' }),
    col.accessor('tenant', { header: 'Tenant' }),
    col.accessor('action_type', { header: 'Type' }),
    col.accessor('verdict', { header: 'Verdict', cell: (info) => <Badge>{info.getValue()}</Badge> }),
    col.accessor('outcome', { header: 'Outcome', cell: (info) => <Badge>{info.getValue()}</Badge> }),
    col.accessor('duration_ms', {
      header: 'Duration',
      cell: (info) => <span className={styles.durationCell}>{info.getValue()}ms</span>,
    }),
    col.accessor('dispatched_at', {
      header: 'Dispatched',
      cell: (info) => <span className={styles.timestampCell} title={info.getValue()}>{relativeTime(info.getValue())}</span>,
    }),
  ]

  return (
    <div>
      <PageHeader title="Audit Trail" />

      <div className={styles.filterBar}>
        <div className={styles.searchInput}>
          <Input
            placeholder="Search by ID..."
            value={searchParams.get('action_id') ?? ''}
            onChange={(e) => setFilter('action_id', e.target.value)}
          />
        </div>
        <Select
          options={[{ value: '', label: 'All Outcomes' }, ...outcomeOptions]}
          value={query.outcome ?? ''}
          onChange={(e) => setFilter('outcome', e.target.value)}
        />
        <Input
          placeholder="Namespace"
          value={query.namespace ?? ''}
          onChange={(e) => setFilter('namespace', e.target.value)}
        />
        <Input
          placeholder="Tenant"
          value={query.tenant ?? ''}
          onChange={(e) => setFilter('tenant', e.target.value)}
        />
      </div>

      <DataTable
        data={data?.records ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={setSelected}
        emptyTitle="No audit records"
        emptyDescription="Actions are recorded when audit is enabled. Dispatch actions to see records here."
        serverTotal={data?.total}
        serverOffset={data?.offset}
        onPageChange={(offset) => setFilter('offset', String(offset))}
      />

      <Drawer open={!!selected} onClose={() => setSelected(null)} title={`Action ${selected?.action_id.slice(0, 12) ?? ''}`} wide>
        {selected && (
          <div>
            <div className={styles.tabContainer}>
              <Tabs
                tabs={[
                  { id: 'overview', label: 'Overview' },
                  { id: 'payload', label: 'Payload' },
                  { id: 'details', label: 'Details' },
                ]}
                active={detailTab}
                onChange={setDetailTab}
                size="sm"
              />
            </div>

            {detailTab === 'overview' && (
              <div className={styles.detailContent}>
                {Object.entries({
                  'Action ID': selected.action_id,
                  'Namespace': selected.namespace,
                  'Tenant': selected.tenant,
                  'Provider': selected.provider,
                  'Action Type': selected.action_type,
                  'Verdict': selected.verdict,
                  'Matched Rule': selected.matched_rule ?? '-',
                  'Outcome': selected.outcome,
                  'Duration': `${selected.duration_ms}ms`,
                  'Dispatched': selected.dispatched_at,
                  'Caller': selected.caller_id,
                  'Auth Method': selected.auth_method,
                }).map(([k, v]) => (
                  <div key={k} className={styles.detailRow}>
                    <span className={styles.detailLabel}>{k}</span>
                    <span className={styles.detailValue}>{v}</span>
                  </div>
                ))}
              </div>
            )}

            {detailTab === 'payload' && (
              <div className={styles.jsonViewerCard}>
                <JsonViewer data={selected.action_payload ?? { message: 'Payload not stored' }} />
              </div>
            )}

            {detailTab === 'details' && (
              <div className={styles.detailsSection}>
                <div>
                  <h3 className={styles.sectionTitle}>Verdict Details</h3>
                  <div className={styles.jsonViewerCard}>
                    <JsonViewer data={selected.verdict_details} />
                  </div>
                </div>
                <div>
                  <h3 className={styles.sectionTitle}>Outcome Details</h3>
                  <div className={styles.jsonViewerCard}>
                    <JsonViewer data={selected.outcome_details} />
                  </div>
                </div>
              </div>
            )}

            <div className={styles.actionContainer}>
              <Button
                variant="secondary"
                size="sm"
                icon={<RotateCcw className="h-3.5 w-3.5" />}
                loading={replay.isPending}
                onClick={() => handleReplay(selected.action_id)}
              >
                Replay
              </Button>
            </div>
          </div>
        )}
      </Drawer>
    </div>
  )
}
