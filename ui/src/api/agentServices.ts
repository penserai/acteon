import { apiResponse } from './client'

const SOURCE_HEADER = 'x-acteon-agent-source-context'
export interface ServiceTask {
  id: string
  namespace: string
  tenant: string
  status: { state: string }
  artifacts?: unknown[]
}
export interface ServiceReceipt {
  readonly namespace: string
  readonly tenant: string
  readonly agent: string
  readonly taskId: string
  readonly sourceContext: string
  readonly task: ServiceTask
}
export type ServiceProviderAbort =
  | { state: 'restricted_only' }
  | { state: 'uncertain'; attemptId: string }
  | { state: 'reconciled'; proofDigest: string }

function providerAbort(value: unknown): ServiceProviderAbort | undefined {
  if (value === undefined || value === null) return undefined
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Provider abort status is malformed')
  const raw = value as Record<string, unknown>
  const keys = Object.keys(raw).sort().join(',')
  if (raw.state === 'restricted_only' && keys === 'state') return { state: 'restricted_only' }
  if (raw.state === 'uncertain' && keys === 'attempt_id,state' && typeof raw.attempt_id === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(raw.attempt_id)) return { state: 'uncertain', attemptId: raw.attempt_id }
  if (raw.state === 'reconciled' && keys === 'proof_digest,state' && typeof raw.proof_digest === 'string' && /^[0-9a-f]{64}$/.test(raw.proof_digest)) return { state: 'reconciled', proofDigest: raw.proof_digest }
  throw new Error('Provider abort status is malformed')
}
function segment(value: string): string {
  if (!value || value === '.' || value === '..') throw new Error('Invalid service route')
  return encodeURIComponent(value)
}
function base(namespace: string, tenant: string, agent: string): string {
  return `/a2a/${segment(namespace)}/${segment(tenant)}/agents/${segment(agent)}/v1`
}
function source(value: string | null): string {
  if (!value || value.length > 8192 || !/^[A-Za-z0-9_-]+$/.test(value)) throw new Error('Admission receipt unavailable. Retry the same request to recover it.')
  return value
}
async function task(response: Response, namespace: string, tenant: string, id?: string): Promise<ServiceTask> {
  if (response.headers.get('a2a-version') !== '1.0') throw new Error('Unsupported agent service response')
  const value = await response.json() as ServiceTask
  if (!value || !value.id || value.namespace !== namespace || value.tenant !== tenant || (id !== undefined && value.id !== id)) throw new Error('Agent service task identity mismatch')
  return value
}
export async function sendServiceMessage(namespace: string, tenant: string, agent: string, messageId: string, text: string): Promise<ServiceReceipt> {
  const response = await apiResponse(base(namespace, tenant, agent) + '/message:send', {
    method: 'POST', redirect: 'error',
    headers: { 'a2a-version': '1.0' },
    body: JSON.stringify({ message: { role: 'user', messageId, parts: [{ kind: 'text', text }] } }),
  })
  const accepted = await task(response, namespace, tenant)
  return Object.freeze({ namespace, tenant, agent, taskId: accepted.id, task: accepted, sourceContext: source(response.headers.get(SOURCE_HEADER)) })
}
export async function observeServiceTask(receipt: ServiceReceipt): Promise<ServiceTask> {
  const response = await apiResponse(base(receipt.namespace, receipt.tenant, receipt.agent) + '/tasks/' + segment(receipt.taskId), {
    redirect: 'error', headers: { 'a2a-version': '1.0', [SOURCE_HEADER]: source(receipt.sourceContext) },
  })
  return task(response, receipt.namespace, receipt.tenant, receipt.taskId)
}

export async function stopServiceTask(receipt: ServiceReceipt): Promise<{ task: ServiceTask; futureStartsBlocked: true; providerAbort?: ServiceProviderAbort }> {
  const response = await apiResponse(base(receipt.namespace, receipt.tenant, receipt.agent) + '/tasks/' + segment(receipt.taskId) + '/stop', {
    method: 'POST', redirect: 'error', headers: { 'a2a-version': '1.0', [SOURCE_HEADER]: source(receipt.sourceContext) },
  })
  if (response.headers.get('a2a-version') !== '1.0') throw new Error('Unsupported agent service response')
  const value = await response.json() as { task?: ServiceTask; future_starts_blocked?: unknown; provider_abort?: unknown } | null
  if (!value || value.future_starts_blocked !== true) throw new Error('Stop acknowledgement unavailable. Retry the stop for this same task.')
  const stopped = value.task
  if (!stopped || stopped.id !== receipt.taskId || stopped.namespace !== receipt.namespace || stopped.tenant !== receipt.tenant) throw new Error('Agent service task identity mismatch')
  const abort = providerAbort(value.provider_abort)
  return { task: stopped, futureStartsBlocked: true, ...(abort ? { providerAbort: abort } : {}) }
}
