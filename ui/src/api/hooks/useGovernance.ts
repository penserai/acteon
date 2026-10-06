import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type { GovernanceChangeReceipt, GovernanceInterventionRequest, GovernanceScopeView, ProviderExecutionHistory, PublishGovernancePermitRequest } from '../../types'

export function useGovernance(namespace: string, tenant: string) {
  return useQuery({
    queryKey: ['governance', namespace, tenant],
    queryFn: () => apiGet<GovernanceScopeView>('/v1/governance', { namespace, tenant }),
    enabled: !!namespace && !!tenant,
    retry: false,
  })
}
export function useGovernanceIntervention() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (request: GovernanceInterventionRequest) => apiPost<GovernanceChangeReceipt>('/v1/governance/changes', request),
    retry: false,
    onSuccess: () => void client.invalidateQueries({ queryKey: ['governance'] }),
  })
}
export function usePublishGovernancePermit() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (request: PublishGovernancePermitRequest) => apiPost<GovernanceChangeReceipt>('/v1/governance/permits', request),
    retry: false,
    onSuccess: () => void client.invalidateQueries({ queryKey: ['governance'] }),
  })
}

export function useProviderExecutionHistory(namespace: string, tenant: string, executionId: string) {
  return useQuery({
    queryKey: ['provider-history', namespace, tenant, executionId],
    queryFn: () => apiGet<ProviderExecutionHistory>(`/v1/governance/executions/${encodeURIComponent(executionId)}`, { namespace, tenant }),
    enabled: !!namespace && !!tenant && !!executionId,
    retry: false,
  })
}
