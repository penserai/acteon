import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type { AuditPage, AuditRecord, AuditQuery, ReplayResult, ReplaySummary } from '../../types'

export function useAudit(query: AuditQuery) {
  return useQuery({
    queryKey: ['audit', query],
    queryFn: () => apiGet<AuditPage>('/v1/audit', query as Record<string, string | number | undefined>),
  })
}

export function useAuditRecord(actionId: string | undefined) {
  return useQuery({
    queryKey: ['audit', actionId],
    queryFn: () => apiGet<AuditRecord>(`/v1/audit/${actionId}`),
    enabled: !!actionId,
  })
}

export function useReplayAction() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (actionId: string) =>
      apiPost<ReplayResult>(`/v1/audit/${actionId}/replay`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['audit'] }),
  })
}

export function useBulkReplay() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (query: AuditQuery) =>
      apiPost<ReplaySummary>('/v1/audit/replay', query),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['audit'] }),
  })
}
