import { useState } from 'react'
import { useCircuitBreakers, useTripCircuit, useResetCircuit } from '../api/hooks/useCircuitBreakers'
import { PageHeader } from '../components/layout/PageHeader'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Drawer } from '../components/ui/Drawer'
import { Modal } from '../components/ui/Modal'
import { EmptyState } from '../components/ui/EmptyState'
import { Skeleton } from '../components/ui/Skeleton'
import { useToast } from '../components/ui/useToast'
import { cn } from '../lib/cn'
import type { CircuitBreakerStatus } from '../types'
import { Zap, RotateCcw, Server } from 'lucide-react'
import styles from './Providers.module.css'

export function Providers() {
  const { data: circuits, isLoading, error } = useCircuitBreakers()
  const trip = useTripCircuit()
  const reset = useResetCircuit()
  const { toast } = useToast()
  const [selected, setSelected] = useState<CircuitBreakerStatus | null>(null)
  const [confirm, setConfirm] = useState<{ action: 'trip' | 'reset'; provider: string } | null>(null)

  const handleConfirm = () => {
    if (!confirm) return
    const mutation = confirm.action === 'trip' ? trip : reset
    mutation.mutate(confirm.provider, {
      onSuccess: () => {
        toast('success', `Circuit ${confirm.action === 'trip' ? 'tripped' : 'reset'}`)
        setConfirm(null)
        setSelected(null)
      },
      onError: (e) => toast('error', `Failed to ${confirm.action}`, (e as Error).message),
    })
  }

  if (isLoading) {
    return (
      <div>
        <PageHeader title="Providers" />
        <div className={styles.loadingGrid}>
          {Array.from({ length: 4 }).map((_, i) => <Skeleton key={i} className="h-40" />)}
        </div>
      </div>
    )
  }

  return (
    <div>
      <PageHeader title="Providers" />

      {error ? (
        <EmptyState
          icon={<Server className="h-12 w-12" />}
          title="Circuit breakers not enabled"
          description="Enable circuit breakers in your acteon.toml to see provider status."
        />
      ) : !circuits || circuits.length === 0 ? (
        <EmptyState
          icon={<Server className="h-12 w-12" />}
          title="No providers configured"
          description="Add providers to your acteon.toml configuration."
        />
      ) : (
        <div className={styles.providersGrid}>
          {circuits.map((cb) => (
            <button
              key={cb.provider}
              onClick={() => setSelected(cb)}
              className={cn(styles.providerCard)}
            >
              <div className={styles.cardHeader}>
                <span className={styles.providerName}>{cb.provider}</span>
                <Badge>{cb.state}</Badge>
              </div>
              <div className={styles.cardMetadata}>
                <p>Failure threshold: {cb.failure_threshold}</p>
                <p>Recovery timeout: {cb.recovery_timeout_seconds}s</p>
                {cb.fallback_provider && <p>Fallback: {cb.fallback_provider}</p>}
              </div>
            </button>
          ))}
        </div>
      )}

      <Drawer open={!!selected} onClose={() => setSelected(null)} title={selected?.provider ?? ''}>
        {selected && (
          <div className={styles.detailContent}>
            <div>
              <h3 className={styles.sectionTitle}>Circuit Breaker</h3>
              <div className={styles.detailsGrid}>
                <div className={styles.detailRow}>
                  <span className={styles.detailLabel}>State</span>
                  <Badge size="md">{selected.state}</Badge>
                </div>
                <div className={styles.detailRow}>
                  <span className={styles.detailLabel}>Failure Threshold</span>
                  <span>{selected.failure_threshold}</span>
                </div>
                <div className={styles.detailRow}>
                  <span className={styles.detailLabel}>Success Threshold</span>
                  <span>{selected.success_threshold}</span>
                </div>
                <div className={styles.detailRow}>
                  <span className={styles.detailLabel}>Recovery Timeout</span>
                  <span>{selected.recovery_timeout_seconds}s</span>
                </div>
                {selected.fallback_provider && (
                  <div className={styles.detailRow}>
                    <span className={styles.detailLabel}>Fallback</span>
                    <span className={styles.fallbackValue}>{selected.fallback_provider}</span>
                  </div>
                )}
              </div>
            </div>

            <div className={styles.actionButtons}>
              <Button
                variant="danger"
                size="sm"
                icon={<Zap className="h-3.5 w-3.5" />}
                onClick={() => setConfirm({ action: 'trip', provider: selected.provider })}
              >
                Trip Circuit
              </Button>
              <Button
                variant="success"
                size="sm"
                icon={<RotateCcw className="h-3.5 w-3.5" />}
                onClick={() => setConfirm({ action: 'reset', provider: selected.provider })}
              >
                Reset Circuit
              </Button>
            </div>
          </div>
        )}
      </Drawer>

      <Modal
        open={!!confirm}
        onClose={() => setConfirm(null)}
        title={confirm?.action === 'trip' ? 'Trip Circuit' : 'Reset Circuit'}
        footer={
          <>
            <Button variant="secondary" onClick={() => setConfirm(null)}>Cancel</Button>
            <Button
              variant={confirm?.action === 'trip' ? 'danger' : 'success'}
              loading={trip.isPending || reset.isPending}
              onClick={handleConfirm}
            >
              Confirm
            </Button>
          </>
        }
      >
        {confirm?.action === 'trip'
          ? <p>Force-open circuit for <strong>{confirm.provider}</strong>? All actions will be rejected or routed to fallback.</p>
          : <p>Force-close circuit for <strong>{confirm?.provider}</strong>? Normal operation will resume.</p>}
      </Modal>
    </div>
  )
}
