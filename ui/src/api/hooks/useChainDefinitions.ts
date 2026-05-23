import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPut, apiDelete } from '../client'
import type { ChainDefinitionListResponse, ChainDefinition } from '../../types'

export function useChainDefinitions() {
  return useQuery({
    queryKey: ['chain-definitions'],
    queryFn: () => apiGet<ChainDefinitionListResponse>('/v1/chains/definitions'),
    refetchInterval: 30000,
  })
}

export function useChainDefinition(name: string | undefined) {
  return useQuery({
    queryKey: ['chain-definitions', name],
    queryFn: () => apiGet<ChainDefinition>(`/v1/chains/definitions/${name}`),
    enabled: !!name,
  })
}

export function useSaveChainDefinition() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (config: ChainDefinition) =>
      apiPut<ChainDefinition>(`/v1/chains/definitions/${config.name}`, config),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['chain-definitions'] }),
  })
}

export function useDeleteChainDefinition() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (name: string) => apiDelete(`/v1/chains/definitions/${name}`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['chain-definitions'] }),
  })
}
