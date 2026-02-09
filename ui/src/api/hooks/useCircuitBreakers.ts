import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type { CircuitBreakerStatus } from '../../types'

export function useCircuitBreakers() {
  return useQuery({
    queryKey: ['circuit-breakers'],
    queryFn: async () => {
      const res = await apiGet<{ circuit_breakers: CircuitBreakerStatus[] }>('/admin/circuit-breakers')
      return res.circuit_breakers
    },
    refetchInterval: 15000,
  })
}

export function useTripCircuit() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (provider: string) =>
      apiPost(`/admin/circuit-breakers/${encodeURIComponent(provider)}/trip`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['circuit-breakers'] }),
  })
}

export function useResetCircuit() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (provider: string) =>
      apiPost(`/admin/circuit-breakers/${encodeURIComponent(provider)}/reset`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['circuit-breakers'] }),
  })
}
