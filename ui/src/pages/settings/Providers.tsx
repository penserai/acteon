import { useConfig } from '../../api/hooks/useConfig'
import { Badge } from '../../components/ui/Badge'
import { Skeleton } from '../../components/ui/Skeleton'
import styles from './Settings.module.css'

function formatSeconds(seconds: number): string {
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`
  return `${Math.floor(seconds / 86400)}d`
}

export function SettingsProviders() {
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
        View registered action providers and circuit breaker configuration.
      </p>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Registered Providers</h3>
        {config.providers.length === 0 ? (
          <p className={styles.valueMuted}>No providers registered</p>
        ) : (
          <table className={styles.providerTable}>
            <thead>
              <tr>
                <th className={styles.providerHeader}>Name</th>
                <th className={styles.providerHeader}>Type</th>
                <th className={styles.providerHeader}>URL</th>
              </tr>
            </thead>
            <tbody>
              {config.providers.map((provider) => (
                <tr key={provider.name}>
                  <td className={styles.providerCell}>{provider.name}</td>
                  <td className={styles.providerCell}>{provider.provider_type}</td>
                  <td className={styles.providerCell}>
                    {provider.url ?? <span className={styles.valueMuted}>N/A</span>}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Circuit Breaker</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Enabled</span>
            <span className={styles.enabledBadge}>
              {config.circuit_breaker.enabled ? (
                <Badge variant="success">Enabled</Badge>
              ) : (
                <Badge variant="error">Disabled</Badge>
              )}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Failure Threshold</span>
            <span className={styles.value}>{config.circuit_breaker.failure_threshold}</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Success Threshold</span>
            <span className={styles.value}>{config.circuit_breaker.success_threshold}</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Recovery Timeout</span>
            <span className={styles.value}>
              {formatSeconds(config.circuit_breaker.recovery_timeout_seconds)}
            </span>
          </div>
        </div>
      </div>

      {config.circuit_breaker.provider_overrides.length > 0 && (
        <div className={styles.card}>
          <h3 className={styles.cardTitle}>Per-Provider Overrides</h3>
          <div className={styles.grid}>
            {config.circuit_breaker.provider_overrides.map((name) => (
              <div key={name} className={styles.row}>
                <span className={styles.value}>{name}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
