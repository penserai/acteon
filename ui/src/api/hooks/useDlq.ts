import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type { DlqStats } from '../../types'

export function useDlqStats() {
  return useQuery({
    queryKey: ['dlq-stats'],
    queryFn: () => apiGet<DlqStats>('/v1/dlq/stats'),
    refetchInterval: 15000,
  })
}

export function useDrainDlq() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: () => apiPost('/v1/dlq/drain'),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['dlq-stats'] }),
  })
}
