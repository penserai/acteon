import { useMutation, useQueryClient } from '@tanstack/react-query'
import { apiPost } from '../client'
import type { DispatchRequest, DispatchResponse } from '../../types'

/**
 * Parse a raw `ActionOutcome` enum from the server into a structured response.
 *
 * The server returns the Rust enum as JSON:
 * - Unit variants: `"Deduplicated"`
 * - Struct/tuple variants: `{"Failed": {"code": "...", ...}}`
 */
function parseOutcome(
  actionId: string,
  raw: unknown,
): DispatchResponse {
  // Unit variant — plain string like "Deduplicated"
  if (typeof raw === 'string') {
    return { action_id: actionId, outcome: raw, details: null }
  }

  // Struct/tuple variant — object with a single key like {"Failed": {...}}
  if (typeof raw === 'object' && raw !== null) {
    const keys = Object.keys(raw)
    if (keys.length === 1) {
      const outcome = keys[0]
      const inner = (raw as Record<string, unknown>)[outcome]
      const details =
        typeof inner === 'object' && inner !== null
          ? (inner as Record<string, unknown>)
          : { value: inner }
      return { action_id: actionId, outcome, details }
    }
  }

  // Fallback — show raw response
  return {
    action_id: actionId,
    outcome: 'Unknown',
    details: typeof raw === 'object' && raw !== null ? (raw as Record<string, unknown>) : null,
  }
}

export function useDispatch() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: async ({
      request,
      dryRun,
    }: {
      request: DispatchRequest
      dryRun?: boolean
    }): Promise<DispatchResponse> => {
      const actionId = crypto.randomUUID()
      const raw = await apiPost<unknown>(
        `/v1/dispatch${dryRun ? '?dry_run=true' : ''}`,
        {
          id: actionId,
          created_at: new Date().toISOString(),
          ...request,
        },
      )
      return parseOutcome(actionId, raw)
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['metrics'] })
      void qc.invalidateQueries({ queryKey: ['audit'] })
    },
  })
}
