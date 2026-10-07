import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { apiFetch, apiGet, apiPost } from '../client'
import type { GovernanceChangeReceipt, GovernanceInterventionRequest, GovernanceRegistryMutationReceipt, GovernanceRegistryMutationRequest, GovernanceRegistryProjection, GovernanceRegistryProjectionView, GovernanceScopeView, ProviderExecutionHistory, PublishGovernancePermitRequest } from '../../types'

type RegistryProjectionTarget = { namespace: string; tenant: string; agentId: string; projection: GovernanceRegistryProjection }
const isObject = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value)
function parseRegistryProjection(value: unknown, target: RegistryProjectionTarget): GovernanceRegistryProjectionView {
  if (!isObject(value) || !isObject(value.agent_resource)
    || value.namespace !== target.namespace || value.tenant !== target.tenant || value.agent_id !== target.agentId || value.projection !== target.projection
    || value.agent_resource.kind !== 'agent' || value.agent_resource.namespace !== target.namespace || value.agent_resource.tenant !== target.tenant || value.agent_resource.id !== target.agentId
    || !Number.isSafeInteger(value.registry_revision) || (value.registry_revision as number) < 0
    || ((value.registry_revision as number) === 0) !== (value.qualification_retired === null)
    || (value.qualification_retired !== null && typeof value.qualification_retired !== 'boolean')
    || (value.version !== null && (!Number.isSafeInteger(value.version) || (value.version as number) <= 0))
    || (value.version === null) !== (value.value === null)
    || (value.value !== null && !isObject(value.value))) throw new Error('Registry observation identity or version mismatch')
  return value as unknown as GovernanceRegistryProjectionView
}
function parseRegistryReceipt(value: unknown, request: GovernanceRegistryMutationRequest): GovernanceRegistryMutationReceipt {
  if (!isObject(value) || value.namespace !== request.namespace || value.tenant !== request.tenant || value.agent_id !== request.agent_id
    || value.change_id !== request.change_id || value.projection !== request.projection
    || value.expected_registry_revision !== request.expected_registry_revision || !Number.isSafeInteger(value.expected_registry_revision)
    || (value.expected_registry_revision as number) < 0 || value.delivery_complete !== true || value.applied !== true
    || typeof value.actor !== 'string' || !value.actor || typeof value.input_digest !== 'string' || !/^[0-9a-f]{64}$/.test(value.input_digest)) {
    throw new Error('Unmatched or incomplete registry mutation receipt')
  }
  return value as unknown as GovernanceRegistryMutationReceipt
}

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

export function useGovernanceRegistryProjection() {
  return useMutation({
    mutationFn: async (target: RegistryProjectionTarget) => {
      const query = new URLSearchParams({ namespace: target.namespace, tenant: target.tenant, projection: target.projection })
      const value = await apiFetch<unknown>(`/v1/governance/registry/${encodeURIComponent(target.agentId)}?${query}`, { redirect: 'error' })
      return parseRegistryProjection(value, target)
    },
    retry: false,
  })
}

export function useGovernanceRegistryMutation() {
  return useMutation({
    mutationFn: async (request: GovernanceRegistryMutationRequest) => {
      const sent = structuredClone(request)
      const value = await apiFetch<unknown>('/v1/governance/registry', {
        method: 'POST', redirect: 'error', body: JSON.stringify(sent),
      })
      return parseRegistryReceipt(value, sent)
    },
    retry: false,
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
