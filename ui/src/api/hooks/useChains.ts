import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type {
  ChainSummary,
  ChainDetailResponse,
  ChainHistoryResponse,
  DagResponse,
  ExecutionHistoryResponse,
} from '../../types'

export function useChains(params: { status?: string }) {
  return useQuery({
    queryKey: ['chains', params],
    queryFn: async () => {
      const resp = await apiGet<{ chains: ChainSummary[] }>('/v1/chains', params)
      return resp.chains
    },
  })
}

export function useChainDetail(
  chainId: string | undefined,
) {
  return useQuery({
    queryKey: ['chain', chainId],
    queryFn: () => apiGet<ChainDetailResponse>(`/v1/chains/${chainId}`),
    enabled: !!chainId,
    refetchInterval: (query) => {
      const data = query.state.data as ChainDetailResponse | undefined
      return data?.status === 'running' || data?.status === 'waiting_sub_chain' ? 2000 : false
    },
  })
}

export function useChainDag(
  chainId: string | undefined,
) {
  return useQuery({
    queryKey: ['chain-dag', chainId],
    queryFn: () =>
      apiGet<DagResponse>(`/v1/chains/${chainId}/dag`),
    enabled: !!chainId,
    refetchInterval: 5000,
  })
}

export function useChainDefinitionDag(name: string | undefined) {
  return useQuery({
    queryKey: ['chain-definition-dag', name],
    queryFn: () => apiGet<DagResponse>(`/v1/chains/definitions/${name}/dag`),
    enabled: !!name,
  })
}

export function useChainHistory(
  chainId: string | undefined,
) {
  return useQuery({
    queryKey: ['chain-history', chainId],
    queryFn: () => apiGet<ChainHistoryResponse>(`/v1/chains/${chainId}/history`),
    enabled: !!chainId,
  })
}

export function useExecutionHistory(
  executionId: string | undefined,
) {
  return useQuery({
    queryKey: ['execution-history', executionId],
    queryFn: () =>
      apiGet<ExecutionHistoryResponse>(`/v1/executions/${executionId}/history`),
    enabled: !!executionId,
    refetchInterval: 5000,
  })
}

export function useSignalExecution() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ executionId, signalName, payload }: {
      executionId: string; signalName: string; payload?: unknown
    }) => apiPost(`/v1/executions/${executionId}/signal/${signalName}`, { payload }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['chain'] })
      void qc.invalidateQueries({ queryKey: ['execution-history'] })
    },
  })
}

export function useCancelChain() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ chainId, reason }: {
      chainId: string; reason?: string
    }) => apiPost(`/v1/chains/${chainId}/cancel`, { reason }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['chains'] })
      void qc.invalidateQueries({ queryKey: ['chain'] })
    },
  })
}
