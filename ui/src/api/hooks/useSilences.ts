import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost, apiPut, apiDelete } from '../client'
import type { CreateSilenceRequest, Silence, UpdateSilenceRequest } from '../../types'

export interface ListSilencesParams {
  namespace?: string
  tenant?: string
  includeExpired?: boolean
}

export function useSilences(params: ListSilencesParams = {}) {
  return useQuery({
    queryKey: ['silences', params],
    queryFn: async () => {
      const res = await apiGet<{ silences: Silence[]; count: number }>('/v1/silences', {
        namespace: params.namespace,
        tenant: params.tenant,
        include_expired: params.includeExpired ? 'true' : undefined,
      })
      return res.silences
    },
  })
}

export function useSilenceDetail(id: string | undefined) {
  return useQuery({
    queryKey: ['silence', id],
    queryFn: () => apiGet<Silence>(`/v1/silences/${encodeURIComponent(id!)}`),
    enabled: !!id,
  })
}

export function useCreateSilence() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: CreateSilenceRequest) => apiPost<Silence>('/v1/silences', body),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['silences'] }),
  })
}

export function useUpdateSilence() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: UpdateSilenceRequest }) =>
      apiPut<Silence>(`/v1/silences/${encodeURIComponent(id)}`, body),
    onSuccess: (_, vars) => {
      void qc.invalidateQueries({ queryKey: ['silences'] })
      void qc.invalidateQueries({ queryKey: ['silence', vars.id] })
    },
  })
}

export function useExpireSilence() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => apiDelete(`/v1/silences/${encodeURIComponent(id)}`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['silences'] }),
  })
}
