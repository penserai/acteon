import { create } from 'zustand'
import { openEventStream } from '../api/sse'
import { shortTime } from '../lib/format'
import type { StreamEvent, MetricsResponse } from '../types'

export type MetricsPoint = Record<string, unknown> & {
  time: string
  executed: number
  failed: number
  suppressed: number
  silenced: number
  muted: number
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
      silenced: m.silenced ?? 0,
      muted: m.muted ?? 0,
      deduplicated: m.deduplicated,
    }
    return {
      metricsHistory: [...prev.slice(-59), point]
    }
  }),
  setStatus: (status) => set({ status }),
  clear: () => set({ events: [], metricsHistory: [] }),
}))

let source: AbortController | null = null
let reconnectTimer: ReturnType<typeof setTimeout> | null = null

export function connectEvents() {
  if (source) return
  useEventStore.getState().setStatus('connecting')
  source = openEventStream('/v1/stream', {
    open: () => useEventStore.getState().setStatus('connected'),
    event: (event) => {
      if (!SSE_EVENT_TYPES.some(type => type === event.type)) return
      try {
        const data = JSON.parse(event.data) as StreamEvent
        data.event_type ||= event.type
        useEventStore.getState().addEvent(data)
      } catch { /* Ignore malformed frames. */ }
    },
    error: () => {
      useEventStore.getState().setStatus('disconnected')
      source?.abort()
      source = null
      reconnectTimer = setTimeout(() => {
        reconnectTimer = null
        connectEvents()
      }, 3000)
    },
  })
}

export function disconnectEvents() {
  source?.abort()
  source = null
  if (reconnectTimer) {
    clearTimeout(reconnectTimer)
    reconnectTimer = null
  }
  useEventStore.getState().setStatus('disconnected')
}
