import { useIdentity } from '../../api/hooks/useIdentity'
import { useConfig } from '../../api/hooks/useConfig'
import { Badge } from '../../components/ui/Badge'
import { Skeleton } from '../../components/ui/Skeleton'
import styles from './Settings.module.css'

export function SettingsAuth() {
  const { data: config, isLoading } = useConfig()
  const { data: identity, error: identityError } = useIdentity()

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
        <h3 className={styles.cardTitle}>Your Identity</h3>
        {identityError ? <p className={styles.description}>Identity could not be loaded.</p> : identity ? (
          <div className={styles.grid}>
            <div className={styles.row}><span className={styles.label}>Credential</span><span>{identity.credential_id || 'Anonymous'}</span></div>
            <div className={styles.row}><span className={styles.label}>Role</span><span>{identity.role}</span></div>
            <div className={styles.row}><span className={styles.label}>Principal</span><span>{identity.principal?.id ?? 'Not configured'}</span></div>
            {identity.principal && <div className={styles.row}><span className={styles.label}>Kind</span><span>{identity.principal.kind}</span></div>}
          </div>
        ) : <Skeleton className="h-16" />}
        <p className={styles.description}>A stable principal identifies the same actor across credential rotation. Roles and scoped grants determine access.</p>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Execution and Administration</h3>
        <p className={styles.description}>
          Give agent runtimes and action-producing services the executor role with
          scoped grants. Executors can submit granted actions but cannot change
          rules, quotas, agent registrations, or other administrative controls.
          Keep operator and admin credentials with trusted administrators.
          Viewer credentials provide observation access subject to endpoint grants.
        </p>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Configuration</h3>
        <p className={styles.description}>
          Auth policies define scoped grants and caller credentials. When file watch
          is enabled, changes are automatically reloaded and apply to subsequent
          API-key and JWT requests. Removing a user invalidates that user's existing sessions.
        </p>
      </div>
    </div>
  )
}
