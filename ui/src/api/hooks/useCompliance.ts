import { useQuery, useMutation } from '@tanstack/react-query'
import { apiGet, apiPost } from '../client'
import type { ComplianceStatus, HashChainVerification } from '../../types'

export function useComplianceStatus() {
  return useQuery({
    queryKey: ['compliance-status'],
    queryFn: () => apiGet<ComplianceStatus>('/v1/compliance/status'),
    refetchInterval: 30000,
  })
}

export function useVerifyChain() {
  return useMutation({
    mutationFn: (params: { namespace: string; tenant: string }) =>
      apiPost<HashChainVerification>('/v1/compliance/verify-chain', params),
  })
}
