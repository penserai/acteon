import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost, apiPut, apiDelete } from '../client'
import type {
  RecurringAction,
  RecurringActionListResponse,
  CreateRecurringActionRequest,
  CreateRecurringActionResponse,
  UpdateRecurringActionRequest,
  PauseResumeResponse,
} from '../../types'

export function useRecurringActions(params: { namespace?: string; tenant?: string; enabled?: string }) {
  return useQuery({
    queryKey: ['recurring', params],
    queryFn: () => apiGet<RecurringActionListResponse>('/v1/recurring', params),
    enabled: !!params.namespace && !!params.tenant,
    refetchInterval: 30000,
  })
}

export function useRecurringAction(id: string | undefined, params: { namespace?: string; tenant?: string }) {
  return useQuery({
    queryKey: ['recurring', id, params],
    queryFn: () => apiGet<RecurringAction>(`/v1/recurring/${id}`, params),
    enabled: !!id && !!params.namespace && !!params.tenant,
    refetchInterval: 30000,
  })
}

export function useCreateRecurring() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateRecurringActionRequest) =>
      apiPost<CreateRecurringActionResponse>('/v1/recurring', request),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['recurring'] }),
  })
}

export function useUpdateRecurring() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, body }: {
      id: string; body: UpdateRecurringActionRequest
    }) => apiPut<RecurringAction>(`/v1/recurring/${id}`, body),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['recurring'] }),
  })
}

export function useDeleteRecurring() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, namespace, tenant }: { id: string; namespace: string; tenant: string }) =>
      apiDelete(`/v1/recurring/${id}?namespace=${encodeURIComponent(namespace)}&tenant=${encodeURIComponent(tenant)}`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['recurring'] }),
  })
}

export function usePauseRecurring() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, namespace, tenant }: { id: string; namespace: string; tenant: string }) =>
      apiPost<PauseResumeResponse>(`/v1/recurring/${id}/pause`, { namespace, tenant }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['recurring'] }),
  })
}

export function useResumeRecurring() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, namespace, tenant }: { id: string; namespace: string; tenant: string }) =>
      apiPost<PauseResumeResponse>(`/v1/recurring/${id}/resume`, { namespace, tenant }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['recurring'] }),
  })
}
