import { openEventStream } from '../sse'
import { useEffect, useRef, useCallback, useState } from 'react'
import type { StreamEvent } from '../../types'

interface StreamOptions {
  action_type?: string
  event_type?: string
  onEvent: (event: StreamEvent) => void
  enabled?: boolean
}

/** SSE event type names emitted by the server. */
const SSE_EVENT_TYPES = [
  'action_dispatched',
  'action_status_changed',
  'group_flushed',
  'timeout',
  'chain_advanced',
  'approval_required',
  'scheduled_action_due',
  'chain_step_completed',
  'chain_completed',
  'group_event_added',
  'group_resolved',
  'approval_resolved',
] as const

export function useStream({ action_type, event_type, onEvent, enabled = true }: StreamOptions) {
  const [status, setStatus] = useState<'connecting' | 'connected' | 'disconnected'>('disconnected')
  const sourceRef = useRef<AbortController | null>(null)
  const onEventRef = useRef(onEvent)
  useEffect(() => {
    onEventRef.current = onEvent
  }, [onEvent])

  const connectRef = useRef<() => void>(() => {})

  const connect = useCallback(() => {
    if (!enabled) return

    const params = new URLSearchParams()
    if (action_type) params.set('action_type', action_type)
    if (event_type) params.set('event_type', event_type)

    setStatus('connecting')
    sourceRef.current?.abort()
    sourceRef.current = openEventStream(`/v1/stream?${params.toString()}`, {
      open: () => setStatus('connected'),
      event: event => {
        if (!SSE_EVENT_TYPES.includes(event.type as typeof SSE_EVENT_TYPES[number])) return
        try {
          const data = JSON.parse(event.data) as StreamEvent
          if (!data.event_type) data.event_type = event.type
          onEventRef.current(data)
        } catch { /* Ignore malformed events. */ }
      },
      error: () => {
        setStatus('disconnected')
        reconnectRef.current = setTimeout(() => connectRef.current(), 3000)
      },
    })
  }, [action_type, event_type, enabled])

  useEffect(() => {
    connectRef.current = connect
  }, [connect])

  const reconnectRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    const timer = setTimeout(() => connect(), 0)
    return () => {
      clearTimeout(timer)
      sourceRef.current?.abort()
      sourceRef.current = null
      if (reconnectRef.current) clearTimeout(reconnectRef.current)
    }
  }, [connect])

  const disconnect = useCallback(() => {
    sourceRef.current?.abort()
    sourceRef.current = null
    if (reconnectRef.current) clearTimeout(reconnectRef.current)
    setStatus('disconnected')
  }, [])

  return { status, disconnect }
}
