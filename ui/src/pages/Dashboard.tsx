import { useRef, useState, useCallback } from 'react'
import { useNavigate } from 'react-router-dom'
import { RefreshCw } from 'lucide-react'
import { useMetrics } from '../api/hooks/useHealth'
import { useCircuitBreakers } from '../api/hooks/useCircuitBreakers'
import { useStream } from '../api/hooks/useStream'
import { PageHeader } from '../components/layout/PageHeader'
import { StatCard } from '../components/charts/StatCard'
import { TimeSeriesChart } from '../components/charts/TimeSeriesChart'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { StatCardSkeleton } from '../components/ui/Skeleton'
import { shortTime } from '../lib/format'
import type { StreamEvent, MetricsResponse } from '../types'
import styles from './Dashboard.module.css'

export function Dashboard() {
  const navigate = useNavigate()
  const { data: metrics, isLoading, refetch } = useMetrics()
  const { data: circuits } = useCircuitBreakers()
  const [events, setEvents] = useState<StreamEvent[]>([])
  const historyRef = useRef<Record<string, unknown>[]>([])

  // Accumulate time series
  if (metrics) {
    const now = new Date()
    const point = {
      time: shortTime(now),
      executed: metrics.executed,
      failed: metrics.failed,
      suppressed: metrics.suppressed,
      deduplicated: metrics.deduplicated,
    }
    if (historyRef.current.length === 0 || historyRef.current[historyRef.current.length - 1].time !== point.time) {
      historyRef.current = [...historyRef.current.slice(-59), point]
    }
  }

  const handleEvent = useCallback((event: StreamEvent) => {
    setEvents((prev) => [event, ...prev].slice(0, 20))
  }, [])

  useStream({
    onEvent: handleEvent,
    enabled: true,
  })

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
              data={historyRef.current}
              series={[
                { key: 'executed', color: '#10B981', label: 'Executed' },
                { key: 'failed', color: '#EF4444', label: 'Failed' },
                { key: 'suppressed', color: '#F59E0B', label: 'Suppressed' },
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

function buildStatCards(m: MetricsResponse) {
  return [
    { label: 'Dispatched', value: m.dispatched, link: '/audit' },
    { label: 'Executed', value: m.executed, link: '/audit?outcome=Executed' },
    { label: 'Failed', value: m.failed, link: '/audit?outcome=Failed' },
    { label: 'Deduplicated', value: m.deduplicated },
    { label: 'Suppressed', value: m.suppressed },
    { label: 'Pending Approval', value: m.pending_approval ?? 0, link: '/approvals' },
    { label: 'Circuit Open', value: m.circuit_open ?? 0, link: '/circuit-breakers' },
    { label: 'Scheduled', value: m.scheduled ?? 0, link: '/scheduled' },
  ]
}
