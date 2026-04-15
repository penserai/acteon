import { useEffect, useRef } from 'react'
import { useNavigate } from 'react-router-dom'
import { RefreshCw } from 'lucide-react'
import { useMetrics } from '../api/hooks/useHealth'
import { useCircuitBreakers } from '../api/hooks/useCircuitBreakers'
import { useEventStore } from '../stores/events'
import { PageHeader } from '../components/layout/PageHeader'
import { StatCard } from '../components/charts/StatCard'
import { TimeSeriesChart } from '../components/charts/TimeSeriesChart'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { StatCardSkeleton } from '../components/ui/Skeleton'
import { shortTime } from '../lib/format'
import type { MetricsResponse } from '../types'
import styles from './Dashboard.module.css'

export function Dashboard() {
  const navigate = useNavigate()
  const { data: metrics, isLoading, refetch } = useMetrics()
  const { data: circuits } = useCircuitBreakers()
  const allEvents = useEventStore((s) => s.events)
  const events = allEvents.slice(0, 10)
  const history = useEventStore((s) => s.metricsHistory)

  // Push polled metrics into the event store for the time-series chart
  const addMetricsPoint = useRef(useEventStore.getState().addMetricsPoint)
  useEffect(() => {
    if (metrics) addMetricsPoint.current(metrics)
  }, [metrics])

  const statCards = metrics ? buildStatCards(metrics) : []

  return (
    <div>
      <PageHeader
        title="Dashboard"
        actions={
          <Button variant="secondary" size="sm" icon={<RefreshCw className="h-3.5 w-3.5" />} onClick={() => void refetch()}>
            Refresh
          </Button>
        }
      />

      {isLoading ? (
        <div className={styles.loadingGrid}>
          {Array.from({ length: 8 }).map((_, i) => <StatCardSkeleton key={i} />)}
        </div>
      ) : (
        <>
          <div className={styles.statGrid}>
            {statCards.map((card) => (
              <StatCard
                key={card.label}
                label={card.label}
                value={card.value}
                onClick={() => card.link && navigate(card.link)}
              />
            ))}
          </div>

          <div className={styles.chartCard}>
            <h2 className={styles.sectionTitle}>Actions Over Time</h2>
            <TimeSeriesChart
              data={history}
              series={[
                { key: 'executed', color: '#10B981', label: 'Executed' },
                { key: 'failed', color: '#EF4444', label: 'Failed' },
                { key: 'suppressed', color: '#F59E0B', label: 'Suppressed' },
                { key: 'silenced', color: '#06B6D4', label: 'Silenced' },
                { key: 'muted', color: '#A855F7', label: 'Muted' },
                { key: 'deduplicated', color: '#64647A', label: 'Deduplicated' },
              ]}
            />
          </div>

          <div className={styles.gridLayout}>
            <div className={styles.card}>
              <h2 className={styles.sectionTitle}>Provider Health</h2>
              {circuits && circuits.length > 0 ? (
                <div className={styles.providerList}>
                  {circuits.map((cb) => (
                    <div key={cb.provider} className={styles.providerItem}>
                      <span className={styles.providerName}>{cb.provider}</span>
                      <Badge>{cb.state}</Badge>
                    </div>
                  ))}
                </div>
              ) : (
                <p className={styles.emptyMessage}>No circuit breakers configured.</p>
              )}
            </div>

            <div className={styles.card}>
              <h2 className={styles.sectionTitle}>Recent Activity</h2>
              {events.length > 0 ? (
                <div className={styles.activityList}>
                  {events.map((evt) => (
                    <div key={evt.id} className={styles.activityItem}>
                      <span className={styles.activityTimestamp}>
                        {shortTime(evt.timestamp)}
                      </span>
                      <Badge size="sm">{evt.event_type}</Badge>
                      <span className={styles.activityType}>{evt.action_type}</span>
                      <span className={styles.activityMeta}>{evt.namespace}/{evt.tenant}</span>
                    </div>
                  ))}
                </div>
              ) : (
                <p className={styles.emptyMessage}>Waiting for events...</p>
              )}
            </div>
          </div>
        </>
      )}
    </div>
  )
}

interface StatCard {
  label: string
  value: number
  link?: string
}

function buildStatCards(m: MetricsResponse): StatCard[] {
  const cards: StatCard[] = [
    { label: 'Dispatched', value: m.dispatched, link: '/audit' },
    { label: 'Executed', value: m.executed, link: '/audit?outcome=Executed' },
    { label: 'Failed', value: m.failed, link: '/audit?outcome=Failed' },
    { label: 'Deduplicated', value: m.deduplicated, link: '/audit?outcome=Deduplicated' },
    { label: 'Suppressed', value: m.suppressed, link: '/audit?outcome=Suppressed' },
    { label: 'Silenced', value: m.silenced ?? 0, link: '/silences' },
    { label: 'Muted', value: m.muted ?? 0, link: '/time-intervals' },
    { label: 'Pending Approval', value: m.pending_approval ?? 0, link: '/approvals' },
    { label: 'Circuit Open', value: m.circuit_open ?? 0, link: '/circuit-breakers' },
    { label: 'Scheduled', value: m.scheduled ?? 0, link: '/scheduled' },
  ]

  // Signing cards render only when signing is actually in use on the
  // server. The signing.reject_* counters all start at 0 on a fresh
  // server; if any of them (or signing_verified) is non-zero, signing
  // is enabled and operators want visibility.
  const sigVerified = m.signing_verified ?? 0
  const sigRejected =
    (m.signing_invalid ?? 0) +
    (m.signing_unknown_signer ?? 0) +
    (m.signing_scope_denied ?? 0) +
    (m.signing_unsigned_rejected ?? 0) +
    (m.signing_replay_rejected ?? 0)
  if (sigVerified > 0 || sigRejected > 0) {
    cards.push({ label: 'Sig Verified', value: sigVerified })
    cards.push({ label: 'Sig Rejected', value: sigRejected })
  }

  return cards
}
