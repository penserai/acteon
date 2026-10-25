import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type { ApprovalStatus } from '../../types'

export function useApprovals() {
  return useQuery({
    queryKey: ['approvals'],
    queryFn: async () => {
      const res = await apiGet<{ approvals: ApprovalStatus[]; count: number }>('/v1/approvals')
      return res.approvals
    },
    refetchInterval: 10000,
  })
}

export function useApproveAction() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id }: { id: string }) =>
      apiPost(`/v1/approvals/${encodeURIComponent(id)}/approve`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['approvals'] }),
  })
}

export function useRejectAction() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id }: { id: string }) =>
      apiPost(`/v1/approvals/${encodeURIComponent(id)}/reject`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['approvals'] }),
  })
}
