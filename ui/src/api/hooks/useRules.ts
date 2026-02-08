import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost, apiPut } from '../client'
import type { RuleSummary } from '../../types'

export function useRules() {
  return useQuery({
    queryKey: ['rules'],
    queryFn: () => apiGet<RuleSummary[]>('/v1/rules'),
  })
}

export function useReloadRules() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (directory?: string) =>
      apiPost('/v1/rules/reload', directory ? { directory } : undefined),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['rules'] }),
  })
}

export function useToggleRule() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ name, enabled }: { name: string; enabled: boolean }) =>
      apiPut(`/v1/rules/${encodeURIComponent(name)}/enabled`, { enabled }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['rules'] }),
  })
}
