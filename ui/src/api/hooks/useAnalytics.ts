import { useQuery } from '@tanstack/react-query'
import { apiGet } from '../client'
import type { AnalyticsQuery, AnalyticsResponse } from '../../types'

export function useAnalytics(query: AnalyticsQuery) {
  return useQuery({
    queryKey: ['analytics', query],
    queryFn: () =>
      apiGet<AnalyticsResponse>(
        '/v1/analytics',
        query as unknown as Record<string, string | number | undefined>,
      ),
    refetchInterval: 30000,
  })
}
