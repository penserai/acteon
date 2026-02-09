import { useQuery } from '@tanstack/react-query'
import { apiGet } from '../client'
import type { ConfigResponse } from '../../types'

export function useConfig() {
  return useQuery({
    queryKey: ['config'],
    queryFn: () => apiGet<ConfigResponse>('/admin/config'),
    staleTime: 60000,
  })
}
