import { useQuery } from '@tanstack/react-query'
import { apiGet } from '../client'
import type { CredentialIdentity } from '../../types'

export function useIdentity() {
  return useQuery({
    queryKey: ['identity'],
    queryFn: async () => {
      const identity = await apiGet<CredentialIdentity>('/v1/auth/identity')
      if (!identity.scope || typeof identity.scope.namespace !== 'string' || !identity.scope.namespace
        || typeof identity.scope.tenant !== 'string' || !identity.scope.tenant) throw new Error('Authenticated context is unavailable')
      return identity
    },
    retry: false,
  })
}
