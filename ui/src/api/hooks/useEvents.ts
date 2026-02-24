import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type { EventState } from '../../types'

export function useEvents(params: { namespace?: string; tenant?: string }) {
  return useQuery({
    queryKey: ['events', params],
    queryFn: async () => {
      const res = await apiGet<{ count: number; events: EventState[] }>('/v1/events', params)
      return res.events
    },
    enabled: !!params.namespace && !!params.tenant,
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
    mutationFn: ({ fingerprint, targetState }: { fingerprint: string; targetState: string }) =>
      apiPost(`/v1/events/${encodeURIComponent(fingerprint)}/transition`, { target_state: targetState }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['events'] }),
  })
}
