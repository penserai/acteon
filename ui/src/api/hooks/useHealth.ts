import { useQuery } from '@tanstack/react-query'
import { apiGet } from '../client'
import type { HealthResponse, MetricsResponse } from '../../types'

export function useHealth() {
  return useQuery({
    queryKey: ['health'],
    queryFn: () => apiGet<HealthResponse>('/health'),
    refetchInterval: 10000,
  })
}

export function useMetrics() {
  return useQuery({
    queryKey: ['metrics'],
    queryFn: () => apiGet<MetricsResponse>('/metrics'),
    refetchInterval: 5000,
  })
}
