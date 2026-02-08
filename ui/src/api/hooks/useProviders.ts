import { useQuery } from '@tanstack/react-query'
import { apiGet } from '../client'
import type { CircuitBreakerStatus } from '../../types'

export function useProviders() {
  return useQuery({
    queryKey: ['providers'],
    queryFn: async () => {
      const res = await apiGet<{ circuit_breakers: CircuitBreakerStatus[] }>('/admin/circuit-breakers')
      return res.circuit_breakers
    },
    refetchInterval: 15000,
  })
}
