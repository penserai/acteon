import { create } from 'zustand'
import { shortTime } from '../lib/format'
import type { StreamEvent, MetricsResponse } from '../types'

export type MetricsPoint = Record<string, unknown> & {
  time: string
  executed: number
  failed: number
  suppressed: number
  deduplicated: number
}

interface EventState {
  events: StreamEvent[]
  metricsHistory: MetricsPoint[]
  status: 'connecting' | 'connected' | 'disconnected'
  addEvent: (event: StreamEvent) => void
  addMetricsPoint: (m: MetricsResponse) => void
  setStatus: (status: 'connecting' | 'connected' | 'disconnected') => void
  clear: () => void
}

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

export const useEventStore = create<EventState>((set) => ({
  events: [],
  metricsHistory: [],
  status: 'disconnected',
  addEvent: (event) => set((s) => ({
    events: [event, ...s.events].slice(0, 100) // keep last 100 events
  })),
  addMetricsPoint: (m) => set((s) => {
    const time = shortTime(new Date())
    const prev = s.metricsHistory
    if (prev.length > 0 && prev[prev.length - 1].time === time) return s

    const point: MetricsPoint = {
      time,
      executed: m.executed,
      failed: m.failed,
      suppressed: m.suppressed,
      deduplicated: m.deduplicated,
    }
    return {
      metricsHistory: [...prev.slice(-59), point]
    }
  }),
  setStatus: (status) => set({ status }),
  clear: () => set({ events: [], metricsHistory: [] }),
}))

let source: EventSource | null = null
let reconnectTimer: ReturnType<typeof setTimeout> | null = null

export function connectEvents() {
  if (source) return

  const baseUrl = import.meta.env.VITE_API_URL ?? ''
  const url = `${baseUrl}/v1/stream`

  useEventStore.getState().setStatus('connecting')
  source = new EventSource(url)

  source.onopen = () => {
    useEventStore.getState().setStatus('connected')
    if (reconnectTimer) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
  }

  const handler = (e: MessageEvent) => {
    try {
      const data = JSON.parse(e.data) as StreamEvent
      if (!data.event_type) {
        data.event_type = e.type
      }
      useEventStore.getState().addEvent(data)
    } catch {
      // ignore
    }
  }

  for (const eventType of SSE_EVENT_TYPES) {
    source.addEventListener(eventType, handler)
  }

  source.onerror = () => {
    useEventStore.getState().setStatus('disconnected')
    source?.close()
    source = null
    // Auto-reconnect after 3s
    reconnectTimer = setTimeout(() => connectEvents(), 3000)
  }
}

export function disconnectEvents() {
  if (source) {
    source.close()
    source = null
  }
  if (reconnectTimer) {
    clearTimeout(reconnectTimer)
    reconnectTimer = null
  }
  useEventStore.getState().setStatus('disconnected')
}
