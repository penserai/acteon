import { useQuery } from '@tanstack/react-query'
import { apiGet } from '../client'
import type { ProviderHealthStatus } from '../../types'

export function useProviderHealth() {
  return useQuery({
    queryKey: ['provider-health'],
    queryFn: async () => {
      const res = await apiGet<{ providers: ProviderHealthStatus[] }>('/v1/providers/health')
      return res.providers
    },
    refetchInterval: 10000,
  })
}
