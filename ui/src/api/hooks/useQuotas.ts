import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost, apiPut, apiDelete } from '../client'
import type {
  QuotaPolicy,
  QuotaUsage,
  QuotaListResponse,
  CreateQuotaRequest,
  CreateQuotaResponse,
  UpdateQuotaRequest,
} from '../../types'

export function useQuotas(params: { namespace?: string; tenant?: string }) {
  return useQuery({
    queryKey: ['quotas', params],
    queryFn: () => apiGet<QuotaListResponse>('/v1/quotas', params),
    refetchInterval: 30000,
  })
}

export function useQuota(id: string | undefined) {
  return useQuery({
    queryKey: ['quotas', id],
    queryFn: () => apiGet<QuotaPolicy>(`/v1/quotas/${id}`),
    enabled: !!id,
  })
}

export function useQuotaUsage(id: string | undefined) {
  return useQuery({
    queryKey: ['quotas', id, 'usage'],
    queryFn: () => apiGet<QuotaUsage>(`/v1/quotas/${id}/usage`),
    enabled: !!id,
    refetchInterval: 15000,
  })
}

export function useCreateQuota() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateQuotaRequest) =>
      apiPost<CreateQuotaResponse>('/v1/quotas', request),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['quotas'] }),
  })
}

export function useUpdateQuota() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: UpdateQuotaRequest }) =>
      apiPut<QuotaPolicy>(`/v1/quotas/${id}`, body),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['quotas'] }),
  })
}

export function useDeleteQuota() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => apiDelete(`/v1/quotas/${id}`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['quotas'] }),
  })
}
