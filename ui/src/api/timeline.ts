import { apiFetch } from './client'

export interface TimelineScope { namespace: string; tenant: string }
export interface TimelineQuery {
  eventGroup?: string
  eventType?: string
  filter?: string
  from?: string
  to?: string
  equals?: { path: string; value: string | number | boolean | null }[]
  exists?: string[]
  text?: { text: string; path?: string; mode: 'all' | 'any' }
  ascending?: boolean
  limit?: number
  cursor?: string
}
export interface TimelineEvent {
  scope: TimelineScope
  source: string
  eventId: string
  revision: number
  firstIngestedAt: string
  updatedAt: string
  expiresAt: string
  event: {
    eventGroup: string
    occurredAt: string
    eventType: string
    message?: string | null
    attributes: Record<string, unknown>
  }
}
export interface TimelinePage { events: TimelineEvent[]; nextCursor: string | null }
export interface TimelineLedgerStatus {
  records: number
  bytes: number
  pending: number
  oldestReceivedAt: string | null
}

export function queryTimeline(scope: TimelineScope, query: TimelineQuery, signal: AbortSignal) {
  return apiFetch<TimelinePage>('/v1/timeline/query', {
    method: 'POST', body: JSON.stringify({ scope, query }), signal,
  })
}
export function getTimelineLedgerStatus(scope: TimelineScope, signal: AbortSignal) {
  return apiFetch<TimelineLedgerStatus>(`/v1/timeline/ledger/status?${new URLSearchParams({ ...scope })}`, { signal })
}

export function timelineErrorMessage(error: unknown): string {
  let message = error instanceof Error ? error.message : 'Timeline request failed.'
  const response = /^\d{3}: (.*)$/s.exec(message)
  if (response) {
    try {
      const body: unknown = JSON.parse(response[1])
      if (body && typeof body === 'object' && 'error' in body && typeof body.error === 'string') message = body.error
    } catch { /* Preserve non-JSON errors from the server. */ }
  }
  if (message === 'timeline disabled') return 'Event timeline is disabled on this server.'
  if (message === 'scope not authorized') return 'You do not have access to this namespace and tenant.'
  if (message === 'timeline storage unavailable') return 'Timeline storage is unavailable. Try again.'
  return message.replace(/^invalid timeline input: /, '')
}
