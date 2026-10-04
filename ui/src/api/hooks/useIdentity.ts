import { useQuery } from '@tanstack/react-query'
import { apiGet } from '../client'
import type { CredentialIdentity } from '../../types'

export function useIdentity() {
  return useQuery({
    queryKey: ['identity'],
    queryFn: () => apiGet<CredentialIdentity>('/v1/auth/identity'),
    retry: false,
  })
}
