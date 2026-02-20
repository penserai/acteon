import { useMutation, useQueryClient } from '@tanstack/react-query'
import { apiPost } from '../client'
import type { DispatchRequest, DispatchResponse } from '../../types'

export function useDispatch() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ request, dryRun }: { request: DispatchRequest; dryRun?: boolean }) =>
      apiPost<DispatchResponse>(`/v1/dispatch${dryRun ? '?dry_run=true' : ''}`, {
        id: crypto.randomUUID(),
        created_at: new Date().toISOString(),
        ...request,
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['metrics'] })
      void qc.invalidateQueries({ queryKey: ['audit'] })
    },
  })
}
