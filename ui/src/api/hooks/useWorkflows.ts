import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type { ListWorkflowExecutionsResponse } from '../../types'

export function useWorkflowExecutions(params: {
  workflow?: string
  status?: string
  limit?: number
}) {
  return useQuery({
    queryKey: ['workflow-executions', params],
    queryFn: async () => {
      const resp = await apiGet<ListWorkflowExecutionsResponse>(
        '/v1/workflows/executions',
        params,
      )
      return resp.executions
    },
    refetchInterval: 5000,
  })
}

export function useCancelWorkflow() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ executionId, reason }: {
      executionId: string; reason?: string
    }) => apiPost(`/v1/workflows/executions/${executionId}/cancel`, { reason }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['workflow-executions'] })
      void qc.invalidateQueries({ queryKey: ['execution-history'] })
    },
  })
}

export function useSignalWorkflow() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ executionId, signalName, payload }: {
      executionId: string; signalName: string; payload?: unknown
    }) => apiPost(`/v1/workflows/executions/${executionId}/signal/${signalName}`, { payload }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['workflow-executions'] })
      void qc.invalidateQueries({ queryKey: ['execution-history'] })
    },
  })
}
