import { useState } from 'react'
import { Trash2 } from 'lucide-react'
import { useDlqStats, useDrainDlq } from '../api/hooks/useDlq'
import { PageHeader } from '../components/layout/PageHeader'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Modal } from '../components/ui/Modal'
import { EmptyState } from '../components/ui/EmptyState'
import { Skeleton } from '../components/ui/Skeleton'
import { useToast } from '../components/ui/useToast'
import { AlertTriangle } from 'lucide-react'
import styles from './DeadLetterQueue.module.css'

export function DeadLetterQueue() {
  const { data: stats, isLoading } = useDlqStats()
  const drain = useDrainDlq()
  const { toast } = useToast()
  const [confirmOpen, setConfirmOpen] = useState(false)

  const handleDrain = () => {
    drain.mutate(undefined, {
      onSuccess: () => { toast('success', 'DLQ drained'); setConfirmOpen(false) },
      onError: (e) => toast('error', 'Drain failed', (e as Error).message),
    })
  }

  if (isLoading) {
    return (
      <div>
        <PageHeader title="Dead-Letter Queue" />
        <Skeleton className="h-40 w-full" />
      </div>
    )
  }

  return (
    <div>
      <PageHeader
        title="Dead-Letter Queue"
        actions={
          stats && stats.enabled && stats.count > 0 ? (
            <Button
              variant="danger"
              size="sm"
              icon={<Trash2 className="h-3.5 w-3.5" />}
              onClick={() => setConfirmOpen(true)}
            >
              Drain All
            </Button>
          ) : undefined
        }
      />

      {stats && (
        <div className={styles.statsCard}>
          <span className={styles.statsLabel}>Status:</span>
          <Badge variant={stats.enabled ? 'success' : 'neutral'}>{stats.enabled ? 'Enabled' : 'Disabled'}</Badge>
          <span className={styles.statsLabelSpaced}>Entries:</span>
          <span className={styles.statsCount}>{stats.count}</span>
        </div>
      )}

      {!stats?.enabled ? (
        <EmptyState
          icon={<AlertTriangle className="h-12 w-12" />}
          title="DLQ disabled"
          description="Enable the dead-letter queue with executor.dlq_enabled = true in your server configuration."
        />
      ) : stats.count === 0 ? (
        <EmptyState
          icon={<AlertTriangle className="h-12 w-12" />}
          title="Queue is empty"
          description="Failed actions that exhaust retry attempts will appear here."
        />
      ) : (
        <div className={styles.infoText}>
          The DLQ contains {stats.count} entries. Use the Drain All button to clear them.
        </div>
      )}

      <Modal
        open={confirmOpen}
        onClose={() => setConfirmOpen(false)}
        title="Drain Dead-Letter Queue"
        footer={
          <>
            <Button variant="secondary" onClick={() => setConfirmOpen(false)}>Cancel</Button>
            <Button variant="danger" loading={drain.isPending} onClick={handleDrain}>Drain DLQ</Button>
          </>
        }
      >
        <p>Permanently drain all {stats?.count ?? 0} DLQ entries? This cannot be undone.</p>
      </Modal>
    </div>
  )
}
