import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'

export interface SwarmRunMetrics {
  total_actions?: number
  agents_spawned?: number
  agents_completed?: number
  agents_failed?: number
  refinements?: number
  adversarial_rounds?: number
  challenges_raised?: number
  challenges_resolved?: number
  eval_baseline_score?: number | null
  eval_final_score?: number | null
}

export interface SwarmRunSnapshot {
  run_id: string
  plan_id: string
  objective: string
  status: string
  started_at: string
  finished_at?: string | null
  metrics?: SwarmRunMetrics | null
  error?: string | null
  namespace: string
  tenant: string
}

export interface SwarmRunListResponse {
  runs: SwarmRunSnapshot[]
  total: number
}

export function useSwarmRuns(params: {
  namespace?: string
  tenant?: string
  status?: string
  limit?: number
  offset?: number
}) {
  return useQuery({
    queryKey: ['swarm-runs', params],
    queryFn: () => apiGet<SwarmRunListResponse>('/v1/swarm/runs', params),
    refetchInterval: 10_000,
  })
}

export function useSwarmRun(runId: string | undefined) {
  return useQuery({
    queryKey: ['swarm-runs', runId],
    queryFn: () => apiGet<SwarmRunSnapshot>(`/v1/swarm/runs/${runId}`),
    enabled: !!runId,
    refetchInterval: (query) => {
      const data = query.state.data
      if (!data) return 5_000
      return ['completed', 'failed', 'cancelled', 'timed_out'].includes(data.status)
        ? false
        : 5_000
    },
  })
}

export function useCancelSwarmRun() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (runId: string) =>
      apiPost<SwarmRunSnapshot>(`/v1/swarm/runs/${runId}/cancel`, {}),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['swarm-runs'] }),
  })
}
