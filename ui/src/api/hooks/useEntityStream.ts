import { useEffect, useRef, useState, useCallback } from 'react'
import type { StreamEvent } from '../../types'

export type EntityType = 'chain' | 'group' | 'action'
export type EntityStreamStatus = 'connecting' | 'connected' | 'ended' | 'error'

interface EntityStreamOptions {
  entityType: EntityType
  entityId: string | undefined
  namespace?: string
  tenant?: string
  onEvent: (event: StreamEvent) => void
  onEnd?: (reason: string, entityKind: string, entityId: string) => void
  enabled?: boolean
}

interface SubscriptionEndBody {
  reason: string
  entity_type: string
  entity_id: string
}

const SSE_EVENT_TYPES = [
  'action_dispatched',
  'action_status_changed',
  'group_flushed',
  'group_resolved',
  'group_event_added',
  'timeout',
  'chain_advanced',
  'chain_step_completed',
  'chain_completed',
  'approval_required',
  'approval_resolved',
  'scheduled_action_due',
] as const

export function useEntityStream({
  entityType,
  entityId,
  namespace,
  tenant,
  onEvent,
  onEnd,
  enabled = true,
}: EntityStreamOptions) {
  const [status, setStatus] = useState<EntityStreamStatus>('connecting')
  const sourceRef = useRef<EventSource | null>(null)
  const onEventRef = useRef(onEvent)
  const onEndRef = useRef(onEnd)

  useEffect(() => {
    onEventRef.current = onEvent
  }, [onEvent])
  useEffect(() => {
    onEndRef.current = onEnd
  }, [onEnd])

  useEffect(() => {
    if (!enabled || !entityId) return

    const params = new URLSearchParams()
    if (namespace) params.set('namespace', namespace)
    if (tenant) params.set('tenant', tenant)

    const baseUrl = import.meta.env.VITE_API_URL ?? ''
    const url = `${baseUrl}/v1/subscribe/${entityType}/${encodeURIComponent(entityId)}?${params.toString()}`

    // Defer the initial 'connecting' state transition to avoid the
    // react-hooks/set-state-in-effect lint (cascading renders).
    const initTimer = setTimeout(() => setStatus('connecting'), 0)
    const source = new EventSource(url)
    sourceRef.current = source

    source.onopen = () => setStatus('connected')

    const dataHandler = (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data) as StreamEvent
        if (!data.event_type) data.event_type = e.type
        onEventRef.current(data)
      } catch {
        // ignore parse errors
      }
    }

    for (const eventType of SSE_EVENT_TYPES) {
      source.addEventListener(eventType, dataHandler)
    }

    // subscription_end carries a distinct body shape, not a StreamEvent.
    // After it fires the server closes the connection — treat as terminal,
    // do not auto-reconnect.
    source.addEventListener('subscription_end', (e: MessageEvent) => {
      try {
        const body = JSON.parse(e.data) as SubscriptionEndBody
        onEndRef.current?.(body.reason, body.entity_type, body.entity_id)
      } catch {
        onEndRef.current?.('unknown', entityType, entityId)
      }
      setStatus('ended')
      source.close()
      sourceRef.current = null
    })

    source.onerror = () => {
      // EventSource emits onerror on normal close too — only flag as error
      // if we haven't already transitioned to 'ended'.
      setStatus((prev) => (prev === 'ended' ? prev : 'error'))
    }

    return () => {
      clearTimeout(initTimer)
      source.close()
      sourceRef.current = null
    }
  }, [entityType, entityId, namespace, tenant, enabled])

  const disconnect = useCallback(() => {
    sourceRef.current?.close()
    sourceRef.current = null
    setStatus('ended')
  }, [])

  return { status, disconnect }
}
