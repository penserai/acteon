import { useConfig } from '../../api/hooks/useConfig'
import { Badge } from '../../components/ui/Badge'
import { Skeleton } from '../../components/ui/Skeleton'
import { Button } from '../../components/ui/Button'
import { useThemeStore } from '../../stores/theme'
import { Sun, Moon, Monitor } from 'lucide-react'
import styles from './Settings.module.css'

function formatSeconds(seconds: number): string {
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`
  return `${Math.floor(seconds / 86400)}d`
}

export function SettingsServerConfig() {
  const { data: config, isLoading } = useConfig()
  const mode = useThemeStore((s) => s.mode)
  const setMode = useThemeStore((s) => s.setMode)

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
        View server configuration and global settings. Most settings are read from the TOML configuration file.
      </p>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Server</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Host</span>
            <span className={styles.value}>{config.server.host}</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Port</span>
            <span className={styles.value}>{config.server.port}</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>External URL</span>
            <span className={config.server.external_url ? styles.value : styles.valueMuted}>
              {config.server.external_url ?? 'Not set'}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Max SSE Connections</span>
            <span className={config.server.max_sse_connections_per_tenant !== null ? styles.value : styles.valueMuted}>
              {config.server.max_sse_connections_per_tenant ?? 'Unlimited'}
            </span>
          </div>
        </div>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>State Backend</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Backend</span>
            <span className={styles.value}>{config.state.backend}</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Key Prefix</span>
            <span className={config.state.prefix ? styles.value : styles.valueMuted}>
              {config.state.prefix ?? 'None'}
            </span>
          </div>
        </div>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Executor</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Max Retries</span>
            <span className={config.executor.max_retries !== null ? styles.value : styles.valueMuted}>
              {config.executor.max_retries ?? 'Not set'}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Timeout</span>
            <span className={config.executor.timeout_seconds !== null ? styles.value : styles.valueMuted}>
              {config.executor.timeout_seconds !== null ? `${config.executor.timeout_seconds}s` : 'Not set'}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Max Concurrent</span>
            <span className={config.executor.max_concurrent !== null ? styles.value : styles.valueMuted}>
              {config.executor.max_concurrent ?? 'Unlimited'}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Dead Letter Queue</span>
            <span className={styles.enabledBadge}>
              {config.executor.dlq_enabled ? (
                <Badge variant="success">Enabled</Badge>
              ) : (
                <Badge variant="error">Disabled</Badge>
              )}
            </span>
          </div>
        </div>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Rules</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Directory</span>
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

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Audit</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Enabled</span>
            <span className={styles.enabledBadge}>
              {config.audit.enabled ? (
                <Badge variant="success">Enabled</Badge>
              ) : (
                <Badge variant="error">Disabled</Badge>
              )}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Backend</span>
            <span className={styles.value}>{config.audit.backend}</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>TTL</span>
            <span className={config.audit.ttl_seconds !== null ? styles.value : styles.valueMuted}>
              {config.audit.ttl_seconds !== null ? formatSeconds(config.audit.ttl_seconds) : 'No expiration'}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Store Payload</span>
            <span className={styles.enabledBadge}>
              {config.audit.store_payload ? (
                <Badge variant="info">Yes</Badge>
              ) : (
                <Badge variant="neutral">No</Badge>
              )}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Redaction</span>
            <span className={styles.enabledBadge}>
              {config.audit.redact.enabled ? (
                <Badge variant="warning">Enabled</Badge>
              ) : (
                <Badge variant="neutral">Disabled</Badge>
              )}
            </span>
          </div>
          {config.audit.redact.enabled && (
            <>
              <div className={styles.row}>
                <span className={styles.label}>Redacted Fields</span>
                <span className={styles.value}>
                  {config.audit.redact.field_count > 0 ? `${config.audit.redact.field_count} pattern(s)` : 'None'}
                </span>
              </div>
              <div className={styles.row}>
                <span className={styles.label}>Placeholder</span>
                <span className={styles.value}>{config.audit.redact.placeholder}</span>
              </div>
            </>
          )}
        </div>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Chains</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Max Concurrent Advances</span>
            <span className={styles.value}>{config.chains.max_concurrent_advances}</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Completed Chain TTL</span>
            <span className={styles.value}>{formatSeconds(config.chains.completed_chain_ttl_seconds)}</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Defined Chains</span>
            <span className={styles.value}>{config.chains.definitions.length}</span>
          </div>
        </div>
        {config.chains.definitions.length > 0 && (
          <div style={{ marginTop: '1rem' }}>
            <table className={styles.providerTable}>
              <thead>
                <tr>
                  <th className={styles.providerHeader}>Name</th>
                  <th className={styles.providerHeader}>Steps</th>
                  <th className={styles.providerHeader}>Timeout</th>
                </tr>
              </thead>
              <tbody>
                {config.chains.definitions.map((chain) => (
                  <tr key={chain.name}>
                    <td className={styles.providerCell}>{chain.name}</td>
                    <td className={styles.providerCell}>{chain.steps_count}</td>
                    <td className={styles.providerCell}>
                      {chain.timeout_seconds !== null ? formatSeconds(chain.timeout_seconds) : <span className={styles.valueMuted}>None</span>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Theme</h3>
        <p className={styles.description}>Select your preferred color scheme.</p>
        <div className={styles.themeButtons}>
          {([
            { value: 'system' as const, icon: Monitor, label: 'System' },
            { value: 'light' as const, icon: Sun, label: 'Light' },
            { value: 'dark' as const, icon: Moon, label: 'Dark' },
          ]).map((opt) => (
            <Button
              key={opt.value}
              variant={mode === opt.value ? 'primary' : 'secondary'}
              icon={<opt.icon className="h-4 w-4" />}
              onClick={() => setMode(opt.value)}
            >
              {opt.label}
            </Button>
          ))}
        </div>
      </div>
    </div>
  )
}
