import { useConfig } from '../../api/hooks/useConfig'
import { Badge } from '../../components/ui/Badge'
import { Skeleton } from '../../components/ui/Skeleton'
import styles from './Settings.module.css'

export function SettingsAuth() {
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
        Configure authentication and authorization settings. Auth configuration is loaded from a TOML file.
      </p>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Authentication Status</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Enabled</span>
            <span className={styles.enabledBadge}>
              {config.auth.enabled ? (
                <Badge variant="success">Enabled</Badge>
              ) : (
                <Badge variant="error">Disabled</Badge>
              )}
            </span>
          </div>
          {config.auth.watch !== null && (
            <div className={styles.row}>
              <span className={styles.label}>File Watch Mode</span>
              <span className={styles.enabledBadge}>
                {config.auth.watch ? (
                  <Badge variant="info">Active</Badge>
                ) : (
                  <Badge variant="neutral">Inactive</Badge>
                )}
              </span>
            </div>
          )}
        </div>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Configuration</h3>
        <p className={styles.description}>
          Auth policies define tenant-level permissions and caller credentials. When file watch is enabled, changes to the auth configuration file are automatically reloaded.
        </p>
      </div>
    </div>
  )
}
