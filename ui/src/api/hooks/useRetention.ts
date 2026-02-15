import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost, apiPut, apiDelete } from '../client'
import type {
  RetentionPolicy,
  RetentionListResponse,
  CreateRetentionRequest,
  CreateRetentionResponse,
  UpdateRetentionRequest,
} from '../../types'

export function useRetentionPolicies(params: { namespace?: string; tenant?: string }) {
  return useQuery({
    queryKey: ['retention', params],
    queryFn: () => apiGet<RetentionListResponse>('/v1/retention', params),
    refetchInterval: 30000,
  })
}

export function useRetentionPolicy(id: string | undefined) {
  return useQuery({
    queryKey: ['retention', id],
    queryFn: () => apiGet<RetentionPolicy>(`/v1/retention/${id}`),
    enabled: !!id,
  })
}

export function useCreateRetention() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateRetentionRequest) =>
      apiPost<CreateRetentionResponse>('/v1/retention', request),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['retention'] }),
  })
}

export function useUpdateRetention() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: UpdateRetentionRequest }) =>
      apiPut<RetentionPolicy>(`/v1/retention/${id}`, body),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['retention'] }),
  })
}

export function useDeleteRetention() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => apiDelete(`/v1/retention/${id}`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['retention'] }),
  })
}
