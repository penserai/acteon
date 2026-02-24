import { useState } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import { useEvents, useTransitionEvent } from '../api/hooks/useEvents'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Input } from '../components/ui/Input'
import { Drawer } from '../components/ui/Drawer'
import { Button } from '../components/ui/Button'
import { useToast } from '../components/ui/useToast'
import { relativeTime } from '../lib/format'
import type { EventState } from '../types'
import shared from '../styles/shared.module.css'
import styles from './Events.module.css'

const col = createColumnHelper<EventState>()

export function Events() {
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')
  const { data: events, isLoading } = useEvents({ namespace: ns || undefined, tenant: tenant || undefined })
  const transition = useTransitionEvent()
  const { toast } = useToast()
  const [selected, setSelected] = useState<EventState | null>(null)
  const [targetState, setTargetState] = useState('')

  const handleTransition = () => {
    if (!selected || !targetState) return
    transition.mutate(
      { fingerprint: selected.fingerprint, targetState },
      {
        onSuccess: () => { toast('success', 'State transitioned'); setSelected(null) },
        onError: (e) => toast('error', 'Transition failed', (e as Error).message),
      },
    )
  }

  const columns = [
    col.accessor('fingerprint', {
      header: 'Fingerprint',
      cell: (info) => <span className={styles.fingerprint}>{info.getValue()}</span>,
    }),
    col.accessor('state_machine', { header: 'State Machine' }),
    col.accessor('state', { header: 'Current State', cell: (info) => <Badge>{info.getValue()}</Badge> }),
    col.accessor('updated_at', {
      header: 'Updated',
      cell: (info) => <span className={styles.timestamp}>{relativeTime(info.getValue())}</span>,
    }),
    col.accessor('transitioned_by', { header: 'By' }),
  ]

  return (
    <div>
      <PageHeader title="Events" />

      <div className={styles.filterContainer}>
        <Input placeholder="Namespace" value={ns} onChange={(e) => setNs(e.target.value)} />
        <Input placeholder="Tenant" value={tenant} onChange={(e) => setTenant(e.target.value)} />
      </div>

      <DataTable
        data={events ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={setSelected}
        emptyTitle="No events"
        emptyDescription="Events tracked by state machines will appear here."
      />

      <Drawer open={!!selected} onClose={() => setSelected(null)} title={`Event: ${selected?.fingerprint ?? ''}`}>
        {selected && (
          <div className={styles.drawerContent}>
            <div className={styles.detailsGrid}>
              <div className={styles.detailRow}><span className={shared.detailLabel}>Fingerprint</span><span className={shared.detailValue}>{selected.fingerprint}</span></div>
              <div className={styles.detailRow}><span className={shared.detailLabel}>State Machine</span><span>{selected.state_machine ?? '-'}</span></div>
              <div className={styles.detailRow}><span className={shared.detailLabel}>Current State</span><Badge>{selected.state}</Badge></div>
              <div className={styles.detailRow}><span className={shared.detailLabel}>Updated</span><span>{selected.updated_at}</span></div>
              <div className={styles.detailRow}><span className={shared.detailLabel}>Transitioned By</span><span>{selected.transitioned_by}</span></div>
            </div>

            <div className={styles.transitionSection}>
              <h3 className={styles.sectionTitle}>Manual Transition</h3>
              <div className={styles.transitionControls}>
                <Input placeholder="Target state (e.g. acknowledged, resolved)" value={targetState} onChange={(e) => setTargetState(e.target.value)} />
                <Button size="sm" onClick={handleTransition} loading={transition.isPending} disabled={!targetState}>
                  Transition
                </Button>
              </div>
            </div>
          </div>
        )}
      </Drawer>
    </div>
  )
}
