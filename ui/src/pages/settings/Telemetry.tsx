import { useConfig } from '../../api/hooks/useConfig'
import { Badge } from '../../components/ui/Badge'
import { Skeleton } from '../../components/ui/Skeleton'
import styles from './Settings.module.css'

export function SettingsTelemetry() {
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

  const telemetry = config.telemetry

  return (
    <div className={styles.container}>
      <p className={styles.description}>
        Configure OpenTelemetry tracing for distributed observability. Traces are exported to OTLP-compatible backends.
      </p>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Telemetry Status</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Enabled</span>
            <span className={styles.enabledBadge}>
              {telemetry.enabled ? (
                <Badge variant="success">Enabled</Badge>
              ) : (
                <Badge variant="error">Disabled</Badge>
              )}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Endpoint</span>
            <span className={styles.value}>{telemetry.endpoint}</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Service Name</span>
            <span className={styles.value}>{telemetry.service_name}</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Protocol</span>
            <span className={styles.value}>{telemetry.protocol}</span>
          </div>
        </div>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Sampling Configuration</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Sample Ratio</span>
            <span className={styles.value}>{(telemetry.sample_ratio * 100).toFixed(1)}%</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Timeout</span>
            <span className={styles.value}>{telemetry.timeout_seconds}s</span>
          </div>
        </div>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>About OpenTelemetry</h3>
        <p className={styles.description}>
          OpenTelemetry provides vendor-neutral distributed tracing for observing request flows across services. Traces include action dispatch, rule evaluation, provider execution, and chain advancement spans.
        </p>
      </div>
    </div>
  )
}
