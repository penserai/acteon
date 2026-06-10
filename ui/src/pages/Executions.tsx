import { useState } from 'react'
import { useSearchParams, useNavigate } from 'react-router-dom'
import { createColumnHelper } from '@tanstack/react-table'
import { useExecutions } from '../api/hooks/useExecutions'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { relativeTime } from '../lib/format'
import type { ExecutionSummary } from '../types'
import styles from './Executions.module.css'

const STATUS_OPTIONS = [
  { value: '', label: 'All Statuses' },
  { value: 'running', label: 'Running' },
  { value: 'waiting_sub_chain', label: 'Waiting Sub-Chain' },
  { value: 'waiting_parallel', label: 'Waiting Parallel' },
  { value: 'waiting_timer', label: 'Waiting Timer' },
  { value: 'waiting_signal', label: 'Waiting Signal' },
  { value: 'waiting_worker', label: 'Waiting Worker' },
  { value: 'completed', label: 'Completed' },
  { value: 'failed', label: 'Failed' },
  { value: 'cancelled', label: 'Cancelled' },
  { value: 'timed_out', label: 'Timed Out' },
]

const col = createColumnHelper<ExecutionSummary>()

export function Executions() {
  const [searchParams, setSearchParams] = useSearchParams()
  const navigate = useNavigate()
  const [ns, setNs] = useState(searchParams.get('namespace') ?? '')
  const [tenant, setTenant] = useState(searchParams.get('tenant') ?? '')
  const [chainName, setChainName] = useState(searchParams.get('chain_name') ?? '')
  const [attr, setAttr] = useState(searchParams.get('attr') ?? '')
  const statusFilter = searchParams.get('status') ?? ''

  const setParam = (key: string, value: string) => {
    const next = new URLSearchParams(searchParams)
    if (value) next.set(key, value)
    else next.delete(key)
    setSearchParams(next)
  }

  const { data: executions, isLoading } = useExecutions({
    namespace: ns || undefined,
    tenant: tenant || undefined,
    chain_name: chainName.trim() || undefined,
    status: statusFilter || undefined,
    attr: attr.includes('=') ? attr.trim() : undefined,
  })

  const columns = [
    col.accessor('execution_id', {
      header: 'Execution ID',
      cell: (info) => <span className={styles.idCell}>{info.getValue().slice(0, 12)}...</span>,
    }),
    col.accessor('chain_name', { header: 'Chain' }),
    col.accessor('version', {
      header: 'Version',
      cell: (info) => <span className={styles.versionCell}>v{info.getValue()}</span>,
    }),
    col.accessor('status', { header: 'Status', cell: (info) => <Badge>{info.getValue()}</Badge> }),
    col.display({
      id: 'steps',
      header: 'Steps',
      cell: (info) => {
        const row = info.row.original
        return <span className={styles.stepsCell}>{row.current_step}/{row.total_steps}</span>
      },
    }),
    col.accessor('wait_state', {
      header: 'Waiting On',
      cell: (info) => {
        const wait = info.getValue()
        return wait ? <Badge variant="info" size="sm">{wait.kind}</Badge> : <span className={styles.stepsCell}>—</span>
      },
    }),
    col.accessor('started_at', {
      header: 'Started',
      cell: (info) => <span className={styles.timestampCell}>{relativeTime(info.getValue())}</span>,
    }),
    col.accessor('updated_at', {
      header: 'Updated',
      cell: (info) => <span className={styles.timestampCell}>{relativeTime(info.getValue())}</span>,
    }),
  ]

  return (
    <div>
      <PageHeader title="Executions" />

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
          placeholder="Chain name"
          value={chainName}
          onChange={(e) => {
            setChainName(e.target.value)
            setParam('chain_name', e.target.value)
          }}
        />
        <Input
          placeholder="Attribute (key=value)"
          value={attr}
          onChange={(e) => {
            setAttr(e.target.value)
            setParam('attr', e.target.value)
          }}
        />
      </div>

      <DataTable
        data={executions ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={(row) => navigate(`/chains/${row.execution_id}?namespace=${encodeURIComponent(ns)}&tenant=${encodeURIComponent(tenant)}`)}
        emptyTitle="No executions"
        emptyDescription="Enter a namespace and tenant to list chain executions, including terminal ones."
      />
    </div>
  )
}
