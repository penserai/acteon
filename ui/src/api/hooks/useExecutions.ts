import { useQuery } from '@tanstack/react-query'
import { apiGet } from '../client'
import type { ListExecutionsResponse } from '../../types'

export function useExecutions(params: {
  namespace?: string
  tenant?: string
  chain_name?: string
  status?: string
  attr?: string
  limit?: number
}) {
  return useQuery({
    queryKey: ['executions', params],
    queryFn: async () => {
      const resp = await apiGet<ListExecutionsResponse>('/v1/executions', params)
      return resp.executions
    },
    enabled: !!params.namespace && !!params.tenant,
    refetchInterval: 5000,
  })
}
