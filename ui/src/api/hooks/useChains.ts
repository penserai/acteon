import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type { ChainSummary, ChainDetailResponse, DagResponse } from '../../types'

export function useChains(params: { namespace?: string; tenant?: string; status?: string }) {
  return useQuery({
    queryKey: ['chains', params],
    queryFn: () => apiGet<ChainSummary[]>('/v1/chains', params),
    enabled: !!params.namespace && !!params.tenant,
  })
}

export function useChainDetail(chainId: string | undefined) {
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
  params: { namespace: string; tenant: string },
) {
  return useQuery({
    queryKey: ['chain-dag', chainId, params.namespace, params.tenant],
    queryFn: () =>
      apiGet<DagResponse>(`/v1/chains/${chainId}/dag`, {
        namespace: params.namespace,
        tenant: params.tenant,
      }),
    enabled: !!chainId && !!params.namespace && !!params.tenant,
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

export function useCancelChain() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ chainId, namespace, tenant, reason }: {
      chainId: string; namespace: string; tenant: string; reason?: string
    }) => apiPost(`/v1/chains/${chainId}/cancel`, { namespace, tenant, reason }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['chains'] })
      void qc.invalidateQueries({ queryKey: ['chain'] })
    },
  })
}
