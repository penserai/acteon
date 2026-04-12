import { useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { BellOff, BellRing, Layers, Radio, ShieldCheck } from 'lucide-react'
import { useEvents } from '../api/hooks/useEvents'
import { useGroups } from '../api/hooks/useGroups'
import { useSilences } from '../api/hooks/useSilences'
import { useProviderHealth } from '../api/hooks/useProviderHealth'
import { PageHeader } from '../components/layout/PageHeader'
import { StatCard } from '../components/charts/StatCard'
import { Badge } from '../components/ui/Badge'
import { Input } from '../components/ui/Input'
import { Button } from '../components/ui/Button'
import { formatCountdown, relativeTime } from '../lib/format'
import type { ProviderHealthStatus } from '../types'
import styles from './Alerting.module.css'

/**
 * Set of provider names that Acteon treats as alerting-oriented
 * dispatch surfaces (paging, chat, SMS, push, email, webhook).
 *
 * Matching is case-insensitive against the provider name — it
 * catches both bare provider types (`opsgenie`, `slack`) and
 * instance suffixes (`opsgenie-prod`, `slack-ops`).
 */
const ALERTING_PROVIDER_HINTS = [
  'opsgenie',
  'pagerduty',
  'victorops',
  'splunk',
  'pushover',
  'telegram',
  'wechat',
  'slack',
  'discord',
  'teams',
  'twilio',
  'email',
  'webhook',
  'sns',
]

function isAlertingProvider(name: string): boolean {
  const lower = name.toLowerCase()
  return ALERTING_PROVIDER_HINTS.some((h) => lower.includes(h))
}

function providerHealthVariant(
  p: ProviderHealthStatus,
): 'success' | 'warning' | 'error' {
  if (!p.healthy) return 'error'
  if (p.circuit_breaker_state && p.circuit_breaker_state !== 'closed') {
    return 'warning'
  }
  // success_rate is a percentage (0–100), matching
  // `ProviderHealthStatus.success_rate` from the gateway metrics.
  if (p.total_requests > 0 && p.success_rate < 95) return 'warning'
  return 'success'
}

export function Alerting() {
  const navigate = useNavigate()
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')

  const { data: events } = useEvents({
    namespace: ns || undefined,
    tenant: tenant || undefined,
  })
  const { data: groups } = useGroups({
    namespace: ns || undefined,
    tenant: tenant || undefined,
  })
  const { data: silences } = useSilences({
    namespace: ns || undefined,
    tenant: tenant || undefined,
  })
  const { data: providers } = useProviderHealth()

  const activeEvents = useMemo(
    () =>
      (events ?? []).filter(
        (e) => e.state !== 'resolved' && e.state !== 'closed',
      ),
    [events],
  )

  const activeSilences = useMemo(
    () => (silences ?? []).filter((s) => s.active),
    [silences],
  )

  const alertingProviders = useMemo(
    () => (providers ?? []).filter((p) => isAlertingProvider(p.provider)),
    [providers],
  )

  const healthyCount = alertingProviders.filter((p) => p.healthy).length

  const scopeSelected = ns.trim() !== '' && tenant.trim() !== ''

  return (
    <div>
      <PageHeader
        title="Alerting"
        subtitle="Live view of active events, groups, silences, and the providers that deliver them"
      />

      <div className={styles.filterRow}>
        <Input
          placeholder="Namespace (required for events/groups)"
          value={ns}
          onChange={(e) => setNs(e.target.value)}
        />
        <Input
          placeholder="Tenant (required for events/groups)"
          value={tenant}
          onChange={(e) => setTenant(e.target.value)}
        />
      </div>

      <div className={styles.statGrid}>
        <StatCard
          label="Active events"
          value={activeEvents.length}
          onClick={() => navigate('/events')}
        />
        <StatCard
          label="Active groups"
          value={(groups ?? []).length}
          onClick={() => navigate('/groups')}
        />
        <StatCard
          label="Active silences"
          value={activeSilences.length}
          onClick={() => navigate('/silences')}
        />
        <StatCard
          label="Healthy alerting providers"
          value={healthyCount}
          onClick={() => navigate('/provider-health')}
        />
      </div>

      <div className={styles.gridLayout}>
        <section className={styles.card}>
          <div className={styles.cardHeader}>
            <h2 className={styles.sectionTitle}>
              <Radio
                className="h-4 w-4 inline mr-1"
                aria-hidden
              />
              Active events
            </h2>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => navigate('/events')}
            >
              View all
            </Button>
          </div>
          {!scopeSelected ? (
            <p className={styles.empty}>
              Enter a namespace and tenant to load active events.
            </p>
          ) : activeEvents.length === 0 ? (
            <p className={styles.empty}>
              No active events for {ns}/{tenant}.
            </p>
          ) : (
            <div className={styles.cardList}>
              {activeEvents.slice(0, 20).map((e) => (
                <div
                  key={e.fingerprint}
                  className={styles.listRow}
                  onClick={() => navigate('/events')}
                  role="button"
                  tabIndex={0}
                >
                  <div className={styles.rowMain}>
                    <div className={styles.rowLabel}>{e.fingerprint}</div>
                    {e.action_type && (
                      <div className={styles.rowSubtitle}>{e.action_type}</div>
                    )}
                  </div>
                  <Badge
                    variant={e.state === 'active' ? 'error' : 'warning'}
                    size="sm"
                  >
                    {e.state}
                  </Badge>
                </div>
              ))}
            </div>
          )}
        </section>

        <section className={styles.card}>
          <div className={styles.cardHeader}>
            <h2 className={styles.sectionTitle}>
              <BellOff
                className="h-4 w-4 inline mr-1"
                aria-hidden
              />
              Active silences
            </h2>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => navigate('/silences')}
            >
              Manage
            </Button>
          </div>
          {activeSilences.length === 0 ? (
            <p className={styles.empty}>No active silences.</p>
          ) : (
            <div className={styles.cardList}>
              {activeSilences.slice(0, 20).map((s) => (
                <div
                  key={s.id}
                  className={styles.listRow}
                  onClick={() => navigate('/silences')}
                  role="button"
                  tabIndex={0}
                >
                  <div className={styles.rowMain}>
                    <div className={styles.rowLabel}>{s.comment || s.id}</div>
                    <div className={styles.rowSubtitle}>
                      {s.tenant}/{s.namespace} · {s.matchers.length} matcher
                      {s.matchers.length === 1 ? '' : 's'} · by {s.created_by}
                    </div>
                  </div>
                  <span className={styles.rowMeta}>
                    {formatCountdown(s.ends_at)}
                  </span>
                </div>
              ))}
            </div>
          )}
        </section>

        <section className={styles.card}>
          <div className={styles.cardHeader}>
            <h2 className={styles.sectionTitle}>
              <Layers
                className="h-4 w-4 inline mr-1"
                aria-hidden
              />
              Active groups
            </h2>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => navigate('/groups')}
            >
              View all
            </Button>
          </div>
          {!scopeSelected ? (
            <p className={styles.empty}>
              Enter a namespace and tenant to load active groups.
            </p>
          ) : !groups || groups.length === 0 ? (
            <p className={styles.empty}>
              No event groups for {ns}/{tenant}.
            </p>
          ) : (
            <div className={styles.cardList}>
              {groups.slice(0, 20).map((g) => (
                <div
                  key={g.group_id}
                  className={styles.listRow}
                  onClick={() => navigate('/groups')}
                  role="button"
                  tabIndex={0}
                >
                  <div className={styles.rowMain}>
                    <div className={styles.rowLabel}>
                      {g.group_key.slice(0, 24)}…
                    </div>
                    <div className={styles.rowSubtitle}>
                      {g.event_count} event
                      {g.event_count === 1 ? '' : 's'} · notify{' '}
                      {relativeTime(g.notify_at)}
                    </div>
                  </div>
                  <Badge variant="info" size="sm">
                    {g.state}
                  </Badge>
                </div>
              ))}
            </div>
          )}
        </section>

        <section className={styles.card}>
          <div className={styles.cardHeader}>
            <h2 className={styles.sectionTitle}>
              <ShieldCheck
                className="h-4 w-4 inline mr-1"
                aria-hidden
              />
              Alerting provider health
            </h2>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => navigate('/provider-health')}
            >
              Dashboard
            </Button>
          </div>
          {alertingProviders.length === 0 ? (
            <p className={styles.empty}>
              No alerting providers configured. Add one under{' '}
              <a
                href="#"
                className={styles.providerLink}
                onClick={(e) => {
                  e.preventDefault()
                  navigate('/settings/providers')
                }}
              >
                Settings &rarr; Providers
              </a>
              .
            </p>
          ) : (
            <div className={styles.cardList}>
              {alertingProviders.map((p) => (
                <div key={p.provider} className={styles.providerRow}>
                  <span className={styles.providerName}>{p.provider}</span>
                  <div className={styles.sparkRow}>
                    <span className={styles.providerMetrics}>
                      p95 {p.p95_latency_ms.toFixed(0)}ms ·{' '}
                      {p.total_requests > 0
                        ? `${p.success_rate.toFixed(1)}%`
                        : 'no traffic'}
                    </span>
                    <Badge variant={providerHealthVariant(p)} size="sm">
                      {p.healthy ? 'healthy' : 'down'}
                    </Badge>
                  </div>
                </div>
              ))}
            </div>
          )}
        </section>
      </div>

      <div className={styles.quickActions}>
        <Button
          variant="secondary"
          icon={<BellRing className="h-3.5 w-3.5" />}
          onClick={() => navigate('/silences')}
        >
          Create silence
        </Button>
        <Button
          variant="secondary"
          icon={<Radio className="h-3.5 w-3.5" />}
          onClick={() => navigate('/events')}
        >
          Inspect events
        </Button>
        <Button
          variant="secondary"
          icon={<Layers className="h-3.5 w-3.5" />}
          onClick={() => navigate('/groups')}
        >
          Inspect groups
        </Button>
      </div>
    </div>
  )
}
