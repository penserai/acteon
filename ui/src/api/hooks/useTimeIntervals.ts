import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost, apiPut, apiDelete } from '../client'
import type {
  CreateTimeIntervalRequest,
  TimeInterval,
  UpdateTimeIntervalRequest,
} from '../../types'

export function useTimeIntervals() {
  return useQuery({
    queryKey: ['time-intervals'],
    queryFn: async () => {
      const res = await apiGet<{ time_intervals: TimeInterval[]; count: number }>(
        '/v1/time-intervals',
      )
      return res.time_intervals
    },
  })
}

export function useTimeIntervalDetail(
  name: string | undefined,
) {
  return useQuery({
    queryKey: ['time-interval', name],
    queryFn: () =>
      apiGet<TimeInterval>(
        `/v1/time-intervals/${encodeURIComponent(name!)}`,
      ),
    enabled: !!name,
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
      name,
      body,
    }: {
      name: string
      body: UpdateTimeIntervalRequest
    }) =>
      apiPut<TimeInterval>(
        `/v1/time-intervals/${encodeURIComponent(name)}`,
        body,
      ),
    onSuccess: (_, vars) => {
      void qc.invalidateQueries({ queryKey: ['time-intervals'] })
      void qc.invalidateQueries({
        queryKey: ['time-interval', vars.name],
      })
    },
  })
}

export function useDeleteTimeInterval() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({
      name,
    }: {
      name: string
    }) =>
      apiDelete(
        `/v1/time-intervals/${encodeURIComponent(name)}`,
      ),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['time-intervals'] }),
  })
}
