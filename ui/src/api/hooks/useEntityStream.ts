import { openEventStream } from '../sse'
import { useEffect, useRef, useState, useCallback } from 'react'
import type { StreamEvent } from '../../types'

export type EntityType = 'chain' | 'group' | 'action'
export type EntityStreamStatus = 'connecting' | 'connected' | 'ended' | 'error'

interface EntityStreamOptions {
  entityType: EntityType
  entityId: string | undefined
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
  onEvent,
  onEnd,
  enabled = true,
}: EntityStreamOptions) {
  const [status, setStatus] = useState<EntityStreamStatus>('connecting')
  const sourceRef = useRef<AbortController | null>(null)
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

    const initTimer = setTimeout(() => setStatus('connecting'), 0)
    const source = openEventStream(`/v1/subscribe/${entityType}/${encodeURIComponent(entityId)}`, {
      open: () => setStatus('connected'),
      event: event => {
        if (event.type === 'subscription_end') {
          try {
            const body = JSON.parse(event.data) as SubscriptionEndBody
            onEndRef.current?.(body.reason, body.entity_type, body.entity_id)
          } catch { onEndRef.current?.('unknown', entityType, entityId) }
          setStatus('ended')
          source.abort()
          sourceRef.current = null
        } else if (SSE_EVENT_TYPES.includes(event.type as typeof SSE_EVENT_TYPES[number])) {
          try {
            const data = JSON.parse(event.data) as StreamEvent
            if (!data.event_type) data.event_type = event.type
            onEventRef.current(data)
          } catch { /* Ignore malformed events. */ }
        }
      },
      error: () => setStatus(previous => previous === 'ended' ? previous : 'error'),
    })
    sourceRef.current = source

    return () => {
      clearTimeout(initTimer)
      source.abort()
      sourceRef.current = null
    }
  }, [entityType, entityId, enabled])

  const disconnect = useCallback(() => {
    sourceRef.current?.abort()
    sourceRef.current = null
    setStatus('ended')
  }, [])

  return { status, disconnect }
}
