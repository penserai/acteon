import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost, apiPut, apiDelete } from '../client'
import type {
  CreateTimeIntervalRequest,
  TimeInterval,
  UpdateTimeIntervalRequest,
} from '../../types'

export interface ListTimeIntervalsParams {
  namespace?: string
  tenant?: string
}

export function useTimeIntervals(params: ListTimeIntervalsParams = {}) {
  return useQuery({
    queryKey: ['time-intervals', params],
    queryFn: async () => {
      const res = await apiGet<{ time_intervals: TimeInterval[]; count: number }>(
        '/v1/time-intervals',
        {
          namespace: params.namespace,
          tenant: params.tenant,
        },
      )
      return res.time_intervals
    },
  })
}

export function useTimeIntervalDetail(
  namespace: string | undefined,
  tenant: string | undefined,
  name: string | undefined,
) {
  return useQuery({
    queryKey: ['time-interval', namespace, tenant, name],
    queryFn: () =>
      apiGet<TimeInterval>(
        `/v1/time-intervals/${encodeURIComponent(namespace!)}/${encodeURIComponent(
          tenant!,
        )}/${encodeURIComponent(name!)}`,
      ),
    enabled: !!namespace && !!tenant && !!name,
  })
}

export function useCreateTimeInterval() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: CreateTimeIntervalRequest) =>
      apiPost<TimeInterval>('/v1/time-intervals', body),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['time-intervals'] }),
  })
}

export function useUpdateTimeInterval() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({
      namespace,
      tenant,
      name,
      body,
    }: {
      namespace: string
      tenant: string
      name: string
      body: UpdateTimeIntervalRequest
    }) =>
      apiPut<TimeInterval>(
        `/v1/time-intervals/${encodeURIComponent(namespace)}/${encodeURIComponent(
          tenant,
        )}/${encodeURIComponent(name)}`,
        body,
      ),
    onSuccess: (_, vars) => {
      void qc.invalidateQueries({ queryKey: ['time-intervals'] })
      void qc.invalidateQueries({
        queryKey: ['time-interval', vars.namespace, vars.tenant, vars.name],
      })
    },
  })
}

export function useDeleteTimeInterval() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({
      namespace,
      tenant,
      name,
    }: {
      namespace: string
      tenant: string
      name: string
    }) =>
      apiDelete(
        `/v1/time-intervals/${encodeURIComponent(namespace)}/${encodeURIComponent(
          tenant,
        )}/${encodeURIComponent(name)}`,
      ),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['time-intervals'] }),
  })
}
