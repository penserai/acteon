import { useConfig } from '../../api/hooks/useConfig'
import { Badge } from '../../components/ui/Badge'
import { Skeleton } from '../../components/ui/Skeleton'
import styles from './Settings.module.css'

export function SettingsLlm() {
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

  const llm = config.llm_guardrail

  return (
    <div className={styles.container}>
      <p className={styles.description}>
        Configure LLM-based policy evaluation for action guardrails. Policies are defined in rules and can be overridden per action type.
      </p>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>LLM Guardrail Status</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Enabled</span>
            <span className={styles.enabledBadge}>
              {llm.enabled ? (
                <Badge variant="success">Enabled</Badge>
              ) : (
                <Badge variant="error">Disabled</Badge>
              )}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Endpoint</span>
            <span className={styles.value}>{llm.endpoint}</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Model</span>
            <span className={styles.value}>{llm.model}</span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>API Key Configured</span>
            <span className={styles.enabledBadge}>
              {llm.has_api_key ? (
                <Badge variant="success">Yes</Badge>
              ) : (
                <Badge variant="warning">No</Badge>
              )}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Fail Open Mode</span>
            <span className={styles.enabledBadge}>
              {llm.fail_open ? (
                <Badge variant="warning">Enabled</Badge>
              ) : (
                <Badge variant="success">Disabled</Badge>
              )}
            </span>
          </div>
        </div>
      </div>

      <div className={styles.card}>
        <h3 className={styles.cardTitle}>Model Parameters</h3>
        <div className={styles.grid}>
          <div className={styles.row}>
            <span className={styles.label}>Timeout</span>
            <span className={llm.timeout_seconds !== null ? styles.value : styles.valueMuted}>
              {llm.timeout_seconds !== null ? `${llm.timeout_seconds}s` : 'Not set'}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Temperature</span>
            <span className={llm.temperature !== null ? styles.value : styles.valueMuted}>
              {llm.temperature !== null ? llm.temperature : 'Not set'}
            </span>
          </div>
          <div className={styles.row}>
            <span className={styles.label}>Max Tokens</span>
            <span className={llm.max_tokens !== null ? styles.value : styles.valueMuted}>
              {llm.max_tokens !== null ? llm.max_tokens : 'Not set'}
            </span>
          </div>
        </div>
      </div>

      {llm.policy && (
        <div className={styles.card}>
          <h3 className={styles.cardTitle}>Default Policy Preview</h3>
          <pre className={styles.codeBlock}>{llm.policy}</pre>
        </div>
      )}

      {llm.policy_keys.length > 0 && (
        <div className={styles.card}>
          <h3 className={styles.cardTitle}>Per-Action-Type Policies</h3>
          <p className={styles.description}>
            The following action types have custom LLM policies defined in the gateway configuration.
          </p>
          <div className={styles.grid}>
            {llm.policy_keys.map((key) => (
              <div key={key} className={styles.row}>
                <span className={styles.value}>{key}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {config.embedding.enabled && (
        <div className={styles.card}>
          <h3 className={styles.cardTitle}>Embeddings</h3>
          <div className={styles.grid}>
            <div className={styles.row}>
              <span className={styles.label}>Enabled</span>
              <span className={styles.enabledBadge}>
                <Badge variant="success">Enabled</Badge>
              </span>
            </div>
            <div className={styles.row}>
              <span className={styles.label}>Endpoint</span>
              <span className={styles.value}>{config.embedding.endpoint}</span>
            </div>
            <div className={styles.row}>
              <span className={styles.label}>Model</span>
              <span className={styles.value}>{config.embedding.model}</span>
            </div>
            <div className={styles.row}>
              <span className={styles.label}>Topic Cache</span>
              <span className={styles.value}>
                {config.embedding.topic_cache_capacity} entries, {config.embedding.topic_cache_ttl_seconds}s TTL
              </span>
            </div>
            <div className={styles.row}>
              <span className={styles.label}>Text Cache</span>
              <span className={styles.value}>
                {config.embedding.text_cache_capacity} entries, {config.embedding.text_cache_ttl_seconds}s TTL
              </span>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
