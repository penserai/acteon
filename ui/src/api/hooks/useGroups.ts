import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost, apiDelete } from '../client'
import type { EventGroup } from '../../types'

export function useGroups(params: { namespace?: string; tenant?: string }) {
  return useQuery({
    queryKey: ['groups', params],
    queryFn: async () => {
      const res = await apiGet<{ groups: EventGroup[]; total: number }>('/v1/groups', params)
      return res.groups
    },
    enabled: !!params.namespace && !!params.tenant,
  })
}

export function useGroupDetail(groupKey: string | undefined) {
  return useQuery({
    queryKey: ['group', groupKey],
    queryFn: () => apiGet<EventGroup>(`/v1/groups/${encodeURIComponent(groupKey!)}`),
    enabled: !!groupKey,
  })
}

export function useFlushGroup() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (groupKey: string) =>
      apiPost(`/v1/groups/${encodeURIComponent(groupKey)}`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['groups'] }),
  })
}

export function useDeleteGroup() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (groupKey: string) =>
      apiDelete(`/v1/groups/${encodeURIComponent(groupKey)}`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['groups'] }),
  })
}
