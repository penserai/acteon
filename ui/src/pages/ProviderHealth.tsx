import { useState } from 'react'
import { useProviderHealth } from '../api/hooks/useProviderHealth'
import { PageHeader } from '../components/layout/PageHeader'
import { Badge } from '../components/ui/Badge'
import { Drawer } from '../components/ui/Drawer'
import { EmptyState } from '../components/ui/EmptyState'
import { Skeleton } from '../components/ui/Skeleton'
import { cn } from '../lib/cn'
import type { ProviderHealthStatus } from '../types'
import { HeartPulse } from 'lucide-react'
import shared from '../styles/shared.module.css'
import styles from './ProviderHealth.module.css'

function formatLatency(ms: number): string {
  if (ms === 0) return '-'
  if (ms < 1) return `${(ms * 1000).toFixed(0)}us`
  if (ms < 1000) return `${ms.toFixed(1)}ms`
  return `${(ms / 1000).toFixed(2)}s`
}

function formatTimestamp(ts?: number): string {
  if (!ts) return 'Never'
  return new Date(ts).toLocaleString()
}

function formatNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return n.toString()
}

function successRateClass(rate: number): string {
  if (rate >= 99) return styles.successRateHigh
  if (rate >= 95) return styles.successRateMedium
  return styles.successRateLow
}

function healthBadge(provider: ProviderHealthStatus) {
  if (!provider.healthy) return <Badge variant="error">Unhealthy</Badge>
  if (provider.circuit_breaker_state === 'open') return <Badge variant="warning">Circuit Open</Badge>
  if (provider.circuit_breaker_state === 'half_open') return <Badge variant="warning">Half-Open</Badge>
  return <Badge variant="success">Healthy</Badge>
}

export function ProviderHealth() {
  const { data: providers, isLoading, error } = useProviderHealth()
  const [selected, setSelected] = useState<ProviderHealthStatus | null>(null)

  if (isLoading) {
    return (
      <div>
        <PageHeader title="Provider Health" />
        <div className={styles.loadingGrid}>
          {Array.from({ length: 4 }).map((_, i) => <Skeleton key={i} className="h-48" />)}
        </div>
      </div>
    )
  }

  return (
    <div>
      <PageHeader title="Provider Health" />

      {error ? (
        <EmptyState
          icon={<HeartPulse className="h-12 w-12" />}
          title="Unable to load provider health"
          description={(error as Error).message}
        />
      ) : !providers || providers.length === 0 ? (
        <EmptyState
          icon={<HeartPulse className="h-12 w-12" />}
          title="No providers configured"
          description="Add providers to your acteon.toml configuration to see health data."
        />
      ) : (
        <div className={styles.providersGrid}>
          {providers.map((p) => (
            <button
              key={p.provider}
              onClick={() => setSelected(p)}
              className={cn(styles.providerCard)}
            >
              <div className={styles.cardHeader}>
                <span className={styles.providerName}>{p.provider}</span>
                <div className={styles.badges}>
                  {healthBadge(p)}
                </div>
              </div>

              <div className={styles.statsRow}>
                <div className={styles.stat}>
                  <div className={styles.statValue}>{formatNumber(p.total_requests)}</div>
                  <div className={styles.statLabel}>Requests</div>
                </div>
                <div className={styles.stat}>
                  <div className={cn(styles.statValue, successRateClass(p.success_rate))}>
                    {p.total_requests > 0 ? `${p.success_rate.toFixed(1)}%` : '-'}
                  </div>
                  <div className={styles.statLabel}>Success</div>
                </div>
                <div className={styles.stat}>
                  <div className={styles.statValue}>{formatLatency(p.avg_latency_ms)}</div>
                  <div className={styles.statLabel}>Avg Latency</div>
                </div>
              </div>

              <div className={styles.latencyRow}>
                <span>p50: <span className={styles.latencyValue}>{formatLatency(p.p50_latency_ms)}</span></span>
                <span>p95: <span className={styles.latencyValue}>{formatLatency(p.p95_latency_ms)}</span></span>
                <span>p99: <span className={styles.latencyValue}>{formatLatency(p.p99_latency_ms)}</span></span>
              </div>

              {p.last_error && (
                <div className={styles.errorText} title={p.last_error}>
                  Last error: {p.last_error}
                </div>
              )}

              <div className={styles.cardFooter}>
                Last request: {formatTimestamp(p.last_request_at)}
              </div>
            </button>
          ))}
        </div>
      )}

      <Drawer open={!!selected} onClose={() => setSelected(null)} title={selected?.provider ?? ''}>
        {selected && (
          <div className={styles.detailContent}>
            <div>
              <h3 className={styles.sectionTitle}>Health Status</h3>
              <div className={styles.detailsGrid}>
                <div className={styles.detailRow}>
                  <span className={shared.detailLabel}>Status</span>
                  {healthBadge(selected)}
                </div>
                {selected.health_check_error && (
                  <div className={styles.detailRow}>
                    <span className={shared.detailLabel}>Error</span>
                    <span className={styles.detailValue}>{selected.health_check_error}</span>
                  </div>
                )}
                {selected.circuit_breaker_state && (
                  <div className={styles.detailRow}>
                    <span className={shared.detailLabel}>Circuit Breaker</span>
                    <Badge size="md">{selected.circuit_breaker_state}</Badge>
                  </div>
                )}
              </div>
            </div>

            <div>
              <h3 className={styles.sectionTitle}>Performance</h3>
              <div className={cn(styles.successRate, successRateClass(selected.success_rate))}>
                {selected.total_requests > 0 ? `${selected.success_rate.toFixed(2)}%` : 'No data'}
              </div>
              <div className={styles.detailsGrid}>
                <div className={styles.detailRow}>
                  <span className={shared.detailLabel}>Total Requests</span>
                  <span className={styles.detailValue}>{selected.total_requests.toLocaleString()}</span>
                </div>
                <div className={styles.detailRow}>
                  <span className={shared.detailLabel}>Successes</span>
                  <span className={styles.detailValue}>{selected.successes.toLocaleString()}</span>
                </div>
                <div className={styles.detailRow}>
                  <span className={shared.detailLabel}>Failures</span>
                  <span className={styles.detailValue}>{selected.failures.toLocaleString()}</span>
                </div>
              </div>
            </div>

            <div>
              <h3 className={styles.sectionTitle}>Latency</h3>
              <div className={styles.detailsGrid}>
                <div className={styles.detailRow}>
                  <span className={shared.detailLabel}>Average</span>
                  <span className={styles.detailValue}>{formatLatency(selected.avg_latency_ms)}</span>
                </div>
                <div className={styles.detailRow}>
                  <span className={shared.detailLabel}>p50 (Median)</span>
                  <span className={styles.detailValue}>{formatLatency(selected.p50_latency_ms)}</span>
                </div>
                <div className={styles.detailRow}>
                  <span className={shared.detailLabel}>p95</span>
                  <span className={styles.detailValue}>{formatLatency(selected.p95_latency_ms)}</span>
                </div>
                <div className={styles.detailRow}>
                  <span className={shared.detailLabel}>p99</span>
                  <span className={styles.detailValue}>{formatLatency(selected.p99_latency_ms)}</span>
                </div>
              </div>
            </div>

            {selected.last_error && (
              <div>
                <h3 className={styles.sectionTitle}>Last Error</h3>
                <p className={styles.errorText}>{selected.last_error}</p>
              </div>
            )}

            <div className={styles.cardFooter}>
              Last request: {formatTimestamp(selected.last_request_at)}
            </div>
          </div>
        )}
      </Drawer>
    </div>
  )
}
