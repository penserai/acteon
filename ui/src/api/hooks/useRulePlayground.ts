import { useMutation } from '@tanstack/react-query'
import { apiPost } from '../client'
import type { EvaluateRulesRequest, EvaluateRulesResponse } from '../../types'

export function useEvaluateRules() {
  return useMutation({
    mutationFn: (req: EvaluateRulesRequest) =>
      apiPost<EvaluateRulesResponse>('/v1/rules/evaluate', req),
  })
}
