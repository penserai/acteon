import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type { GovernanceChangeReceipt, WorkforceChangeRequest, WorkforceScopeView } from '../../types'

export function useWorkforce(namespace: string, tenant: string) {
  return useQuery({ queryKey: ['workforce', namespace, tenant],
    queryFn: () => apiGet<WorkforceScopeView>('/v1/workforce', { namespace, tenant }),
    enabled: !!namespace && !!tenant, retry: false })
}
export function useWorkforceChange() {
  const client = useQueryClient()
  return useMutation({ mutationFn: (request: WorkforceChangeRequest) => apiPost<GovernanceChangeReceipt>('/v1/workforce/changes', request),
    retry: false, onSuccess: () => {
      void client.invalidateQueries({ queryKey: ['workforce'] })
      void client.invalidateQueries({ queryKey: ['governance'] })
    } })
}
