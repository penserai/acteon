import { useConfig } from '../../api/hooks/useConfig'
import { Badge } from '../../components/ui/Badge'
import { Skeleton } from '../../components/ui/Skeleton'
import styles from './Settings.module.css'

export function SettingsRateLimiting() {
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

  return (
    <div className={styles.container}>
      <p className={styles.description}>
        Configure rate limiting behavior for incoming actions. Rate limits are defined in the rules configuration.
      </p>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Rate Limiting Status</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Enabled</span>
            <span className={styles.enabledBadge}>
              {config.rate_limit.enabled ? (
                <Badge variant="success">Enabled</Badge>
              ) : (
                <Badge variant="error">Disabled</Badge>
              )}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>On Error Behavior</span>
            <span className={styles.value}>{config.rate_limit.on_error}</span>
          </div>
        </div>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Configuration</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Rules Directory</span>
            <span className={config.rules.directory ? styles.value : styles.valueMuted}>
              {config.rules.directory ?? 'Not set'}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Default Timezone</span>
            <span className={config.rules.default_timezone ? styles.value : styles.valueMuted}>
              {config.rules.default_timezone ?? 'UTC'}
            </span>
          </div>
        </div>
      </div>
    </div>
  )
}
