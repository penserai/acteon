import { useState, useCallback } from 'react'
import { useStream } from '../api/hooks/useStream'
import { PageHeader } from '../components/layout/PageHeader'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { shortTime } from '../lib/format'
import { cn } from '../lib/cn'
import type { StreamEvent } from '../types'
import { Pause, Play, Circle } from 'lucide-react'
import styles from './EventStream.module.css'

export function EventStream() {
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')
  const [typeFilter, setTypeFilter] = useState('')
  const [events, setEvents] = useState<StreamEvent[]>([])
  const [paused, setPaused] = useState(false)
  const [buffer, setBuffer] = useState<StreamEvent[]>([])

  const handleEvent = useCallback((event: StreamEvent) => {
    if (paused) {
      setBuffer((prev) => [event, ...prev])
    } else {
      setEvents((prev) => [event, ...prev].slice(0, 200))
    }
  }, [paused])

  const { status } = useStream({
    namespace: ns || undefined,
    tenant: tenant || undefined,
    event_type: typeFilter || undefined,
    onEvent: handleEvent,
    enabled: true,
  })

  const handleResume = () => {
    setEvents((prev) => [...buffer, ...prev].slice(0, 200))
    setBuffer([])
    setPaused(false)
  }

  return (
    <div>
      <PageHeader title="Event Stream" />

      <div className={styles.controlsContainer}>
        <div className={styles.statusIndicator}>
          <Circle className={cn(
            styles.statusIcon,
            status === 'connected' ? styles.statusIconConnected : status === 'connecting' ? styles.statusIconConnecting : styles.statusIconDisconnected,
          )} />
          <span className={styles.statusLabel}>{status}</span>
        </div>

        <Input placeholder="Namespace" value={ns} onChange={(e) => setNs(e.target.value)} className={styles.filterInput} />
        <Input placeholder="Tenant" value={tenant} onChange={(e) => setTenant(e.target.value)} className={styles.filterInput} />
        <Select
          options={[
            { value: '', label: 'All Types' },
            { value: 'ActionDispatched', label: 'ActionDispatched' },
            { value: 'ChainAdvanced', label: 'ChainAdvanced' },
            { value: 'ApprovalRequired', label: 'ApprovalRequired' },
            { value: 'GroupFlushed', label: 'GroupFlushed' },
            { value: 'Timeout', label: 'Timeout' },
            { value: 'ScheduledActionDue', label: 'ScheduledActionDue' },
          ]}
          value={typeFilter}
          onChange={(e) => setTypeFilter(e.target.value)}
        />

        <Button
          variant="secondary"
          size="sm"
          icon={paused ? <Play className="h-3.5 w-3.5" /> : <Pause className="h-3.5 w-3.5" />}
          onClick={() => paused ? handleResume() : setPaused(true)}
        >
          {paused ? `Resume (${buffer.length})` : 'Pause'}
        </Button>
      </div>

      <div className={styles.eventsContainer}>
        {events.length === 0 ? (
          <div className={styles.emptyState}>
            Waiting for events...
          </div>
        ) : (
          <div className={styles.eventsList}>
            {events.map((evt) => (
              <div key={evt.id} className={styles.eventRow}>
                <span className={styles.eventTime}>
                  {shortTime(evt.timestamp)}
                </span>
                <Badge size="sm">{evt.event_type}</Badge>
                <span className={styles.eventType}>{evt.action_type}</span>
                <span className={styles.eventMeta}>{evt.namespace}/{evt.tenant}</span>
                <span className={styles.eventId}>{evt.action_id.slice(0, 8)}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
