import { useConfig } from '../../api/hooks/useConfig'
import { Badge } from '../../components/ui/Badge'
import { Skeleton } from '../../components/ui/Skeleton'
import { Clock, CheckCircle, Calendar } from 'lucide-react'
import styles from './Settings.module.css'

function formatSeconds(seconds: number): string {
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`
  return `${Math.floor(seconds / 86400)}d`
}

export function SettingsBackground() {
  const { data: config, isLoading } = useConfig()

  if (isLoading) {
    return (
      <div className={styles.container}>
        <Skeleton className="h-48" />
      </div>
    )
  }

  if (!config) {
    return null
  }

  const bg = config.background

  const features = [
    {
      name: 'Group Flush',
      enabled: bg.enable_group_flush,
      icon: CheckCircle,
      description: 'Flush completed event groups',
    },
    {
      name: 'Timeout Processing',
      enabled: bg.enable_timeout_processing,
      icon: Clock,
      description: 'Process chain and approval timeouts',
    },
    {
      name: 'Approval Retry',
      enabled: bg.enable_approval_retry,
      icon: CheckCircle,
      description: 'Retry expired approvals',
    },
    {
      name: 'Scheduled Actions',
      enabled: bg.enable_scheduled_actions,
      icon: Calendar,
      description: 'Process scheduled action execution',
    },
  ]

  const intervals = [
    { label: 'Group Flush', value: bg.group_flush_interval_seconds },
    { label: 'Timeout Check', value: bg.timeout_check_interval_seconds },
    { label: 'Cleanup', value: bg.cleanup_interval_seconds },
    { label: 'Scheduled Check', value: bg.scheduled_check_interval_seconds },
  ]

  return (
    <div className={styles.container}>
      <p className={styles.description}>
        Configure background task processing for async workflows. Background tasks handle event groups, timeouts, approvals, and scheduled actions.
      </p>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Background Tasks Status</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Enabled</span>
            <span className={styles.enabledBadge}>
              {bg.enabled ? (
                <Badge variant="success">Enabled</Badge>
              ) : (
                <Badge variant="error">Disabled</Badge>
              )}
            </span>
          </div>
        </div>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Feature Toggles</h3>
        <div className={styles.featureGrid}>
          {features.map((feature) => {
            const Icon = feature.icon
            return (
              <div key={feature.name} className={styles.featureCard}>
                <div className={styles.featureIcon}>
                  <Icon className="h-5 w-5" style={{ color: 'var(--text-muted)' }} />
                </div>
                <div className={styles.featureContent}>
                  <div className={styles.featureName}>{feature.name}</div>
                  <div className={styles.featureDetail}>{feature.description}</div>
                </div>
                <div className={styles.enabledBadge}>
                  {feature.enabled ? (
                    <Badge variant="success">On</Badge>
                  ) : (
                    <Badge variant="neutral">Off</Badge>
                  )}
                </div>
              </div>
            )
          })}
        </div>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Processing Intervals</h3>
        <div className={styles.intervalGrid}>
          {intervals.map((interval) => (
            <div key={interval.label} className={styles.intervalCard}>
              <div className={styles.intervalValue}>{formatSeconds(interval.value)}</div>
              <div className={styles.intervalLabel}>{interval.label}</div>
            </div>
          ))}
        </div>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>About Background Tasks</h3>
        <p className={styles.description}>
          Background tasks run on configurable intervals to process async workflows. Each feature can be independently enabled or disabled. Intervals control how frequently the system checks for pending work.
        </p>
      </div>
    </div>
  )
}
