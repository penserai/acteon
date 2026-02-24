import { useState } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import { useGroups, useFlushGroup, useDeleteGroup } from '../api/hooks/useGroups'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Input } from '../components/ui/Input'
import { Button } from '../components/ui/Button'
import { Drawer } from '../components/ui/Drawer'
import { Modal } from '../components/ui/Modal'
import { useToast } from '../components/ui/useToast'
import { relativeTime } from '../lib/format'
import type { EventGroup } from '../types'
import { Play, Trash2 } from 'lucide-react'
import shared from '../styles/shared.module.css'
import styles from './Groups.module.css'

const col = createColumnHelper<EventGroup>()

export function Groups() {
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')
  const { data: groups, isLoading } = useGroups({ namespace: ns || undefined, tenant: tenant || undefined })
  const flush = useFlushGroup()
  const deleteGroup = useDeleteGroup()
  const { toast } = useToast()
  const [selected, setSelected] = useState<EventGroup | null>(null)
  const [confirm, setConfirm] = useState<{ action: 'flush' | 'delete'; group: EventGroup } | null>(null)

  const handleConfirm = () => {
    if (!confirm) return
    const mutation = confirm.action === 'flush' ? flush : deleteGroup
    mutation.mutate(confirm.group.group_key, {
      onSuccess: () => {
        toast('success', confirm.action === 'flush' ? 'Group flushed' : 'Group deleted')
        setConfirm(null)
        setSelected(null)
      },
      onError: (e) => toast('error', `Failed to ${confirm.action}`, (e as Error).message),
    })
  }

  const columns = [
    col.accessor('group_key', {
      header: 'Group Key',
      cell: (info) => <span className={styles.groupKey}>{info.getValue().slice(0, 12)}...</span>,
    }),
    col.accessor('event_count', { header: 'Events' }),
    col.accessor('state', { header: 'State', cell: (info) => <Badge>{info.getValue()}</Badge> }),
    col.accessor('notify_at', {
      header: 'Notify At',
      cell: (info) => <span className={styles.timestamp}>{relativeTime(info.getValue())}</span>,
    }),
  ]

  return (
    <div>
      <PageHeader title="Event Groups" />

      <div className={styles.filterContainer}>
        <Input placeholder="Namespace" value={ns} onChange={(e) => setNs(e.target.value)} />
        <Input placeholder="Tenant" value={tenant} onChange={(e) => setTenant(e.target.value)} />
      </div>

      <DataTable
        data={groups ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={setSelected}
        emptyTitle="No event groups"
        emptyDescription="Event groups are created when a Group rule action matches."
      />

      <Drawer
        open={!!selected}
        onClose={() => setSelected(null)}
        title={`Group: ${selected?.group_key.slice(0, 12) ?? ''}`}
        footer={
          selected ? (
            <>
              <Button variant="secondary" size="sm" icon={<Play className="h-3.5 w-3.5" />}
                onClick={() => setConfirm({ action: 'flush', group: selected })}>
                Flush
              </Button>
              <Button variant="danger" size="sm" icon={<Trash2 className="h-3.5 w-3.5" />}
                onClick={() => setConfirm({ action: 'delete', group: selected })}>
                Delete
              </Button>
            </>
          ) : undefined
        }
      >
        {selected && (
          <div className={styles.drawerContent}>
            <div className={styles.detailsGrid}>
              <div className={styles.detailRow}><span className={shared.detailLabel}>Group ID</span><span>{selected.group_id}</span></div>
              <div className={styles.detailRow}><span className={shared.detailLabel}>State</span><Badge>{selected.state}</Badge></div>
              <div className={styles.detailRow}><span className={shared.detailLabel}>Events</span><span>{selected.event_count}</span></div>
              <div className={styles.detailRow}><span className={shared.detailLabel}>Notify At</span><span>{selected.notify_at}</span></div>
              <div className={styles.detailRow}><span className={shared.detailLabel}>Created</span><span>{selected.created_at}</span></div>
            </div>
          </div>
        )}
      </Drawer>

      <Modal
        open={!!confirm}
        onClose={() => setConfirm(null)}
        title={confirm?.action === 'flush' ? 'Flush Group' : 'Delete Group'}
        footer={
          <>
            <Button variant="secondary" onClick={() => setConfirm(null)}>Cancel</Button>
            <Button
              variant={confirm?.action === 'delete' ? 'danger' : 'primary'}
              loading={flush.isPending || deleteGroup.isPending}
              onClick={handleConfirm}
            >
              Confirm
            </Button>
          </>
        }
      >
        {confirm?.action === 'flush'
          ? <p>Flush group? This will trigger the group notification immediately.</p>
          : <p>Delete group? This will discard all {confirm?.group.event_count} grouped events. This cannot be undone.</p>}
      </Modal>
    </div>
  )
}
