import { useState, useMemo } from 'react'
import { BarChart3 } from 'lucide-react'
import { BarChart, Bar, XAxis, YAxis, Tooltip, ResponsiveContainer, Legend, Cell } from 'recharts'
import { useAnalytics } from '../api/hooks/useAnalytics'
import { PageHeader } from '../components/layout/PageHeader'
import { TimeSeriesChart } from '../components/charts/TimeSeriesChart'
import { StatCard } from '../components/charts/StatCard'
import { EmptyState } from '../components/ui/EmptyState'
import { Input } from '../components/ui/Input'
import { Skeleton } from '../components/ui/Skeleton'
import { cn } from '../lib/cn'
import type { AnalyticsMetric, AnalyticsInterval } from '../types'
import styles from './Analytics.module.css'

const METRIC_TABS: { metric: AnalyticsMetric; label: string }[] = [
  { metric: 'volume', label: 'Volume' },
  { metric: 'outcome_breakdown', label: 'Outcomes' },
  { metric: 'top_action_types', label: 'Top Actions' },
  { metric: 'latency', label: 'Latency' },
  { metric: 'error_rate', label: 'Error Rate' },
]

const INTERVAL_OPTIONS: { value: AnalyticsInterval; label: string }[] = [
  { value: 'hourly', label: 'Hourly' },
  { value: 'daily', label: 'Daily' },
  { value: 'weekly', label: 'Weekly' },
  { value: 'monthly', label: 'Monthly' },
]

const OUTCOME_COLORS: Record<string, string> = {
  executed: '#22c55e',
  suppressed: '#f59e0b',
  deduplicated: '#3b82f6',
  rerouted: '#8b5cf6',
  throttled: '#f97316',
  failed: '#ef4444',
  pending_approval: '#6b7280',
}

function formatTimestamp(ts: string, interval: AnalyticsInterval): string {
  const d = new Date(ts)
  switch (interval) {
    case 'hourly':
      return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
    case 'daily':
      return d.toLocaleDateString([], { month: 'short', day: 'numeric' })
    case 'weekly':
      return `W${getWeekNumber(d)}`
    case 'monthly':
      return d.toLocaleDateString([], { month: 'short', year: '2-digit' })
  }
}

function getWeekNumber(d: Date): number {
  const start = new Date(d.getFullYear(), 0, 1)
  const diff = d.getTime() - start.getTime()
  return Math.ceil((diff / 86400000 + start.getDay() + 1) / 7)
}

function formatLatencyMs(ms: number | undefined): string {
  if (ms === undefined || ms === 0) return '-'
  if (ms < 1) return `${(ms * 1000).toFixed(0)}us`
  if (ms < 1000) return `${ms.toFixed(1)}ms`
  return `${(ms / 1000).toFixed(2)}s`
}

function formatCompactNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return n.toString()
}

export function Analytics() {
  const [metric, setMetric] = useState<AnalyticsMetric>('volume')
  const [interval, setInterval] = useState<AnalyticsInterval>('daily')
  const [namespace, setNamespace] = useState('')
  const [tenant, setTenant] = useState('')
  const [provider, setProvider] = useState('')
  const [from, setFrom] = useState('')
  const [to, setTo] = useState('')

  const query = useMemo(() => ({
    metric,
    interval,
    namespace: namespace || undefined,
    tenant: tenant || undefined,
    provider: provider || undefined,
    from: from || undefined,
    to: to || undefined,
    top_n: metric === 'top_action_types' ? 10 : undefined,
  }), [metric, interval, namespace, tenant, provider, from, to])

  const { data, isLoading, error } = useAnalytics(query)

  const chartData = useMemo(() => {
    if (!data?.buckets) return []
    return data.buckets.map((b) => ({
      time: formatTimestamp(b.timestamp, interval),
      count: b.count,
      avg_duration_ms: b.avg_duration_ms,
      p50: b.p50_duration_ms,
      p95: b.p95_duration_ms,
      p99: b.p99_duration_ms,
      error_rate: b.error_rate != null ? +(b.error_rate * 100).toFixed(2) : undefined,
      group: b.group,
    }))
  }, [data, interval])

  const summaryStats = useMemo(() => {
    if (!data) return null
    const buckets = data.buckets
    const totalCount = data.total_count
    const avgLatency = buckets.length > 0
      ? buckets.reduce((sum, b) => sum + (b.avg_duration_ms ?? 0), 0) / buckets.length
      : 0
    const avgErrorRate = buckets.length > 0
      ? buckets.reduce((sum, b) => sum + (b.error_rate ?? 0), 0) / buckets.length
      : 0
    const peakCount = buckets.length > 0
      ? Math.max(...buckets.map((b) => b.count))
      : 0
    return { totalCount, avgLatency, avgErrorRate, peakCount }
  }, [data])

  // Build outcome breakdown bar chart data
  const outcomeBarData = useMemo(() => {
    if (metric !== 'outcome_breakdown' || !data?.buckets) return []
    const grouped: Record<string, number> = {}
    for (const b of data.buckets) {
      const key = b.group ?? 'unknown'
      grouped[key] = (grouped[key] ?? 0) + b.count
    }
    return Object.entries(grouped)
      .map(([name, value]) => ({ name, value }))
      .sort((a, b) => b.value - a.value)
  }, [data, metric])

  return (
    <div>
      <PageHeader
        title="Analytics"
        subtitle="Explore action volume, outcomes, latency, and error trends"
      />

      <div className={styles.container}>
        {/* Metric Tabs */}
        <div className={styles.metricTabs}>
          {METRIC_TABS.map((tab) => (
            <button
              key={tab.metric}
              onClick={() => setMetric(tab.metric)}
              className={cn(
                styles.metricTab,
                metric === tab.metric && styles.metricTabActive,
              )}
            >
              {tab.label}
            </button>
          ))}
        </div>

        {/* Filter Bar */}
        <div className={styles.filterBar}>
          <Input
            label="Namespace"
            value={namespace}
            onChange={(e) => setNamespace(e.target.value)}
            placeholder="All"
          />
          <Input
            label="Tenant"
            value={tenant}
            onChange={(e) => setTenant(e.target.value)}
            placeholder="All"
          />
          <Input
            label="Provider"
            value={provider}
            onChange={(e) => setProvider(e.target.value)}
            placeholder="All"
          />
          <div className={styles.selectWrapper}>
            <label className={styles.selectLabel}>Interval</label>
            <select
              value={interval}
              onChange={(e) => setInterval(e.target.value as AnalyticsInterval)}
              className={styles.select}
            >
              {INTERVAL_OPTIONS.map((opt) => (
                <option key={opt.value} value={opt.value}>{opt.label}</option>
              ))}
            </select>
          </div>
          <Input
            label="From"
            type="date"
            value={from}
            onChange={(e) => setFrom(e.target.value)}
          />
          <Input
            label="To"
            type="date"
            value={to}
            onChange={(e) => setTo(e.target.value)}
          />
        </div>

        {/* Loading */}
        {isLoading && (
          <>
            <div className={styles.loadingGrid}>
              {Array.from({ length: 4 }).map((_, i) => (
                <Skeleton key={i} className="h-24" />
              ))}
            </div>
            <Skeleton className="h-72" />
          </>
        )}

        {/* Error */}
        {error && !isLoading && (
          <EmptyState
            icon={<BarChart3 className="h-12 w-12" />}
            title="Unable to load analytics"
            description={(error as Error).message}
          />
        )}

        {/* Data */}
        {data && !isLoading && (
          <>
            {/* Summary Cards */}
            {summaryStats && (
              <div className={styles.statsRow}>
                <StatCard
                  label="Total Actions"
                  value={summaryStats.totalCount}
                />
                <StatCard
                  label="Peak Count"
                  value={summaryStats.peakCount}
                />
                <StatCard
                  label="Avg Latency (ms)"
                  value={Math.round(summaryStats.avgLatency)}
                />
                <StatCard
                  label="Avg Error Rate"
                  value={+(summaryStats.avgErrorRate * 100).toFixed(2)}
                />
              </div>
            )}

            {/* Volume chart */}
            {metric === 'volume' && (
              <div className={styles.chartCard}>
                <h3 className={styles.chartTitle}>Action Volume Over Time</h3>
                <TimeSeriesChart
                  data={chartData}
                  series={[
                    { key: 'count', color: 'var(--color-primary-400)', label: 'Actions' },
                  ]}
                />
              </div>
            )}

            {/* Latency chart */}
            {metric === 'latency' && (
              <div className={styles.chartCard}>
                <h3 className={styles.chartTitle}>Latency Percentiles Over Time</h3>
                <TimeSeriesChart
                  data={chartData}
                  series={[
                    { key: 'p50', color: '#22c55e', label: 'p50' },
                    { key: 'p95', color: '#f59e0b', label: 'p95' },
                    { key: 'p99', color: '#ef4444', label: 'p99' },
                  ]}
                />
              </div>
            )}

            {/* Error rate chart */}
            {metric === 'error_rate' && (
              <div className={styles.chartCard}>
                <h3 className={styles.chartTitle}>Error Rate Over Time (%)</h3>
                <TimeSeriesChart
                  data={chartData}
                  series={[
                    { key: 'error_rate', color: '#ef4444', label: 'Error Rate %' },
                  ]}
                />
              </div>
            )}

            {/* Outcome breakdown bar chart */}
            {metric === 'outcome_breakdown' && (
              <div className={styles.outcomeChartCard}>
                <h3 className={styles.chartTitle}>Outcome Breakdown</h3>
                {outcomeBarData.length === 0 ? (
                  <div className={styles.emptyState}>No outcome data available</div>
                ) : (
                  <ResponsiveContainer width="100%" height={320}>
                    <BarChart data={outcomeBarData} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                      <XAxis
                        dataKey="name"
                        tick={{ fontSize: 11, fill: 'var(--color-gray-500)' }}
                        axisLine={{ stroke: 'var(--color-gray-200)' }}
                        tickLine={false}
                      />
                      <YAxis
                        tick={{ fontSize: 11, fill: 'var(--color-gray-500)' }}
                        axisLine={false}
                        tickLine={false}
                        width={50}
                      />
                      <Tooltip
                        contentStyle={{
                          backgroundColor: 'var(--color-gray-0)',
                          border: '1px solid var(--color-gray-200)',
                          borderRadius: '6px',
                          fontSize: '13px',
                        }}
                      />
                      <Legend wrapperStyle={{ fontSize: '12px', paddingTop: '8px' }} />
                      <Bar dataKey="value" name="Count" radius={[4, 4, 0, 0]}>
                        {outcomeBarData.map((entry) => (
                          <Cell
                            key={entry.name}
                            fill={OUTCOME_COLORS[entry.name] ?? '#6b7280'}
                          />
                        ))}
                      </Bar>
                    </BarChart>
                  </ResponsiveContainer>
                )}
              </div>
            )}

            {/* Top Action Types table */}
            {metric === 'top_action_types' && (
              <>
                {/* Also show volume chart for context */}
                <div className={styles.chartCard}>
                  <h3 className={styles.chartTitle}>Action Volume Over Time</h3>
                  <TimeSeriesChart
                    data={chartData}
                    series={[
                      { key: 'count', color: 'var(--color-primary-400)', label: 'Actions' },
                    ]}
                  />
                </div>

                <div className={styles.tableCard}>
                  <h3 className={styles.tableTitle}>Top Action Types</h3>
                  {data.top_entries.length === 0 ? (
                    <div className={styles.emptyState}>No action type data available</div>
                  ) : (
                    <table className={styles.table}>
                      <thead>
                        <tr>
                          <th>#</th>
                          <th>Action Type</th>
                          <th>Count</th>
                          <th>Percentage</th>
                        </tr>
                      </thead>
                      <tbody>
                        {data.top_entries.map((entry, idx) => (
                          <tr key={entry.label}>
                            <td className={styles.countCell}>{idx + 1}</td>
                            <td className={styles.labelCell}>{entry.label}</td>
                            <td className={styles.countCell}>{formatCompactNumber(entry.count)}</td>
                            <td>
                              <div className={styles.percentageBar}>
                                <div className={styles.percentageTrack}>
                                  <div
                                    className={styles.percentageFill}
                                    style={{ width: `${Math.min(entry.percentage, 100)}%` }}
                                  />
                                </div>
                                <span className={styles.percentageCell}>
                                  {entry.percentage.toFixed(1)}%
                                </span>
                              </div>
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  )}
                </div>
              </>
            )}

            {/* Latency details table */}
            {metric === 'latency' && data.buckets.length > 0 && (
              <div className={styles.tableCard}>
                <h3 className={styles.tableTitle}>Latency by Period</h3>
                <table className={styles.table}>
                  <thead>
                    <tr>
                      <th>Period</th>
                      <th>Avg</th>
                      <th>p50</th>
                      <th>p95</th>
                      <th>p99</th>
                      <th>Count</th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.buckets.map((b, idx) => (
                      <tr key={idx}>
                        <td className={styles.labelCell}>
                          {formatTimestamp(b.timestamp, interval)}
                        </td>
                        <td className={styles.countCell}>{formatLatencyMs(b.avg_duration_ms)}</td>
                        <td className={styles.countCell}>{formatLatencyMs(b.p50_duration_ms)}</td>
                        <td className={styles.countCell}>{formatLatencyMs(b.p95_duration_ms)}</td>
                        <td className={styles.countCell}>{formatLatencyMs(b.p99_duration_ms)}</td>
                        <td className={styles.countCell}>{formatCompactNumber(b.count)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  )
}
