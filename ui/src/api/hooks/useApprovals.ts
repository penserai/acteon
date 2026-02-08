import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type { ApprovalStatus } from '../../types'

export function useApprovals(params: { namespace?: string; tenant?: string }) {
  return useQuery({
    queryKey: ['approvals', params],
    queryFn: () => apiGet<ApprovalStatus[]>('/v1/approvals', params),
    refetchInterval: 10000,
  })
}

export function useApproveAction() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ ns, tenant, id }: { ns: string; tenant: string; id: string }) =>
      apiPost(`/v1/approvals/${encodeURIComponent(ns)}/${encodeURIComponent(tenant)}/${encodeURIComponent(id)}/approve`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['approvals'] }),
  })
}

export function useRejectAction() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ ns, tenant, id }: { ns: string; tenant: string; id: string }) =>
      apiPost(`/v1/approvals/${encodeURIComponent(ns)}/${encodeURIComponent(tenant)}/${encodeURIComponent(id)}/reject`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['approvals'] }),
  })
}
