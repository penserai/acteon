import { useEffect, useRef, useCallback, useState } from 'react'
import type { StreamEvent } from '../../types'

interface StreamOptions {
  namespace?: string
  tenant?: string
  action_type?: string
  event_type?: string
  onEvent: (event: StreamEvent) => void
  enabled?: boolean
}

/** SSE event type names emitted by the server. */
const SSE_EVENT_TYPES = [
  'action_dispatched',
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

export function useStream({ namespace, tenant, action_type, event_type, onEvent, enabled = true }: StreamOptions) {
  const [status, setStatus] = useState<'connecting' | 'connected' | 'disconnected'>('disconnected')
  const sourceRef = useRef<EventSource | null>(null)
  const onEventRef = useRef(onEvent)
  useEffect(() => {
    onEventRef.current = onEvent
  }, [onEvent])

  const connectRef = useRef<() => void>(() => {})

  const connect = useCallback(() => {
    if (!enabled) return

    const params = new URLSearchParams()
    if (namespace) params.set('namespace', namespace)
    if (tenant) params.set('tenant', tenant)
    if (action_type) params.set('action_type', action_type)
    if (event_type) params.set('event_type', event_type)

    const baseUrl = import.meta.env.VITE_API_URL ?? ''
    const url = `${baseUrl}/v1/stream?${params.toString()}`

    setStatus('connecting')
    const source = new EventSource(url)
    sourceRef.current = source

    source.onopen = () => setStatus('connected')

    // The server sends named SSE events (e.g. `event: action_dispatched`).
    // EventSource.onmessage only fires for unnamed events, so we must
    // register a listener for each known event type.
    const handler = (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data) as StreamEvent
        // The flattened JSON uses `type` as the discriminator tag, but the
        // UI's StreamEvent interface expects `event_type`. Inject it from
        // the SSE event name so the rest of the UI works correctly.
        if (!data.event_type) {
          data.event_type = e.type
        }
        onEventRef.current(data)
      } catch {
        // ignore parse errors
      }
    }

    for (const eventType of SSE_EVENT_TYPES) {
      source.addEventListener(eventType, handler)
    }

    source.onerror = () => {
      setStatus('disconnected')
      source.close()
      // Auto-reconnect after 3s
      reconnectRef.current = setTimeout(() => connectRef.current(), 3000)
    }
  }, [namespace, tenant, action_type, event_type, enabled])

  useEffect(() => {
    connectRef.current = connect
  }, [connect])

  const reconnectRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    // Defer to avoid setState-in-effect warning
    const timer = setTimeout(() => connect(), 0)
    return () => {
      clearTimeout(timer)
      sourceRef.current?.close()
      sourceRef.current = null
      if (reconnectRef.current) clearTimeout(reconnectRef.current)
    }
  }, [connect])

  const disconnect = useCallback(() => {
    sourceRef.current?.close()
    sourceRef.current = null
    setStatus('disconnected')
  }, [])

  return { status, disconnect }
}
