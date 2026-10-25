import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type { EventState } from '../../types'

export function useEvents() {
  return useQuery({
    queryKey: ['events'],
    queryFn: async () => {
      const res = await apiGet<{ count: number; events: EventState[] }>('/v1/events')
      return res.events
    },
  })
}

export function useEventDetail(fingerprint: string | undefined) {
  return useQuery({
    queryKey: ['event', fingerprint],
    queryFn: () => apiGet<EventState>(`/v1/events/${encodeURIComponent(fingerprint!)}`),
    enabled: !!fingerprint,
  })
}

export function useTransitionEvent() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ fingerprint, to }: { fingerprint: string; to: string }) =>
      apiPost(`/v1/events/${encodeURIComponent(fingerprint)}/transition`, { to }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['events'] }),
  })
}
