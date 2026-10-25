import { useSearchParams, useNavigate } from 'react-router-dom'
import { createDataTableColumnHelper } from '../components/ui/tableFeatures'
import { useChains } from '../api/hooks/useChains'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Select } from '../components/ui/Select'
import { relativeTime } from '../lib/format'
import type { ChainSummary } from '../types'
import styles from './Chains.module.css'

const col = createDataTableColumnHelper<ChainSummary>()

export function Chains() {
  const [searchParams, setSearchParams] = useSearchParams()
  const navigate = useNavigate()
  const statusFilter = searchParams.get('status') ?? ''

  const { data: chains, isLoading } = useChains({
    status: statusFilter || undefined,
  })

  const columns = [
    col.accessor('chain_id', {
      header: 'Chain ID',
      cell: (info) => <span className={styles.chainIdCell}>{info.getValue().slice(0, 12)}...</span>,
    }),
    col.accessor('chain_name', { header: 'Name' }),
    col.accessor('status', { header: 'Status', cell: (info) => <Badge>{info.getValue()}</Badge> }),
    col.display({
      id: 'progress',
      header: 'Progress',
      cell: (info) => {
        const row = info.row.original
        const pct = row.total_steps > 0 ? (row.current_step / row.total_steps) * 100 : 0
        return (
          <div className={styles.progressContainer}>
            <div className={styles.progressBar}>
              <div
                className={styles.progressFill}
                style={{ width: `${pct}%` }}
              />
            </div>
            <span className={styles.progressText}>{row.current_step}/{row.total_steps}</span>
          </div>
        )
      },
    }),
    col.accessor('started_at', {
      header: 'Started',
      cell: (info) => <span className={styles.timestampCell}>{relativeTime(info.getValue())}</span>,
    }),
  ]

  return (
    <div>
      <PageHeader title="Chains" />

      <div className={styles.filterBar}>
        <Select
          options={[
            { value: '', label: 'All Statuses' },
            { value: 'running', label: 'Running' },
            { value: 'waiting_sub_chain', label: 'Waiting Sub-Chain' },
            { value: 'completed', label: 'Completed' },
            { value: 'failed', label: 'Failed' },
            { value: 'cancelled', label: 'Cancelled' },
          ]}
          value={statusFilter}
          onChange={(e) => {
            const next = new URLSearchParams(searchParams)
            if (e.target.value) next.set('status', e.target.value)
            else next.delete('status')
            setSearchParams(next)
          }}
        />
      </div>

      <DataTable
        data={chains ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={(row) => navigate(`/chains/${row.chain_id}`)}
        emptyTitle="No chain executions"
        emptyDescription="Chain executions are created when a rule triggers a Chain action."
      />
    </div>
  )
}
