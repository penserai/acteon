import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiDelete } from '../client'
import type { EventGroup } from '../../types'

export function useGroups() {
  return useQuery({
    queryKey: ['groups'],
    queryFn: async () => {
      const res = await apiGet<{ groups: EventGroup[]; total: number }>('/v1/groups')
      return res.groups
    },
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
      apiDelete(`/v1/groups/${encodeURIComponent(groupKey)}`),
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
