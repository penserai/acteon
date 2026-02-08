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

export function useStream({ namespace, tenant, action_type, event_type, onEvent, enabled = true }: StreamOptions) {
  const [status, setStatus] = useState<'connecting' | 'connected' | 'disconnected'>('disconnected')
  const sourceRef = useRef<EventSource | null>(null)
  const onEventRef = useRef(onEvent)
  onEventRef.current = onEvent

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
    source.onmessage = (e) => {
      try {
        const data = JSON.parse(e.data) as StreamEvent
        onEventRef.current(data)
      } catch {
        // ignore parse errors
      }
    }
    source.onerror = () => {
      setStatus('disconnected')
      source.close()
      // Auto-reconnect after 3s
      setTimeout(() => connect(), 3000)
    }
  }, [namespace, tenant, action_type, event_type, enabled])

  useEffect(() => {
    connect()
    return () => {
      sourceRef.current?.close()
      sourceRef.current = null
    }
  }, [connect])

  const disconnect = useCallback(() => {
    sourceRef.current?.close()
    sourceRef.current = null
    setStatus('disconnected')
  }, [])

  return { status, disconnect }
}
