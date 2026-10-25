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
export interface ListedServiceTask {
  readonly id: string
  readonly status: { state: string }
  readonly artifacts?: unknown[]
  readonly metadata?: Record<string, unknown>
}
export interface ServiceTaskPage {
  readonly tasks: readonly ListedServiceTask[]
  readonly nextPageToken: string
  readonly pageSize: number
  readonly totalSize: number
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
function base(agent: string): string {
  return `/a2a/agents/${segment(agent)}/v1`
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
  const response = await apiResponse(base(agent) + '/message:send', {
    method: 'POST', redirect: 'error',
    headers: { 'a2a-version': '1.0' },
    body: JSON.stringify({ message: { role: 'user', messageId, parts: [{ kind: 'text', text }] } }),
  })
  const accepted = await task(response, namespace, tenant)
  return Object.freeze({ namespace, tenant, agent, taskId: accepted.id, task: accepted, sourceContext: source(response.headers.get(SOURCE_HEADER)) })
}
export async function observeServiceTask(receipt: ServiceReceipt): Promise<ServiceTask> {
  const response = await apiResponse(base(receipt.agent) + '/tasks/' + segment(receipt.taskId), {
    redirect: 'error', headers: { 'a2a-version': '1.0', [SOURCE_HEADER]: source(receipt.sourceContext) },
  })
  return task(response, receipt.namespace, receipt.tenant, receipt.taskId)
}

export async function stopServiceTask(receipt: ServiceReceipt): Promise<{ task: ServiceTask; futureStartsBlocked: true; cancellationConfirmed: boolean; providerAbort?: ServiceProviderAbort }> {
  const response = await apiResponse(base(receipt.agent) + '/tasks/' + segment(receipt.taskId) + '/stop', {
    method: 'POST', redirect: 'error', headers: { 'a2a-version': '1.0', [SOURCE_HEADER]: source(receipt.sourceContext) },
  })
  if (response.headers.get('a2a-version') !== '1.0') throw new Error('Unsupported agent service response')
  const value = await response.json() as { task?: ServiceTask; future_starts_blocked?: unknown; cancellation_confirmed?: unknown; provider_abort?: unknown } | null
  if (!value || value.future_starts_blocked !== true || typeof value.cancellation_confirmed !== 'boolean') throw new Error('Stop acknowledgement unavailable. Retry the stop for this same task.')
  const stopped = value.task
  if (!stopped || stopped.id !== receipt.taskId || stopped.namespace !== receipt.namespace || stopped.tenant !== receipt.tenant) throw new Error('Agent service task identity mismatch')
  if (value.cancellation_confirmed && stopped.status?.state !== 'canceled') throw new Error('Stop acknowledgement unavailable. Retry the stop for this same task.')
  const abort = providerAbort(value.provider_abort)
  return { task: stopped, futureStartsBlocked: true, cancellationConfirmed: value.cancellation_confirmed, ...(abort ? { providerAbort: abort } : {}) }
}

export async function listServiceTasks(agent: string, pageToken = ''): Promise<ServiceTaskPage> {
  if (pageToken.length > 8192) throw new Error('Task page token is too large')
  const requestId = `list:${agent}`
  const response = await apiResponse(`/a2a/agents/${segment(agent)}`, {
    method: 'POST', redirect: 'error', headers: { 'a2a-version': '1.0' },
    body: JSON.stringify({
      jsonrpc: '2.0', id: requestId, method: 'ListTasks',
      params: { pageSize: 25, ...(pageToken ? { pageToken } : {}) },
    }),
  })
  if (response.headers.get('a2a-version') !== '1.0') throw new Error('Unsupported agent service response')
  const rpc = await response.json() as Record<string, unknown>
  if (!rpc || rpc.jsonrpc !== '2.0' || rpc.id !== requestId || !rpc.result || typeof rpc.result !== 'object' || Array.isArray(rpc.result)) throw new Error('Agent service task page identity mismatch')
  const page = rpc.result as Record<string, unknown>
  if (Object.keys(page).sort().join(',') !== 'nextPageToken,pageSize,tasks,totalSize'
    || !Array.isArray(page.tasks) || typeof page.nextPageToken !== 'string' || page.nextPageToken.length > 8192
    || page.pageSize !== 25 || typeof page.totalSize !== 'number' || !Number.isSafeInteger(page.totalSize)
    || page.totalSize < page.tasks.length || page.tasks.length > 25) throw new Error('Agent service task page is malformed')
  const tasks = page.tasks.map(value => {
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Agent service task projection is malformed')
    const listed = value as Record<string, unknown>
    const state = listed.status && typeof listed.status === 'object' && !Array.isArray(listed.status)
      ? (listed.status as Record<string, unknown>).state : undefined
    if (typeof listed.id !== 'string' || listed.id.length === 0 || typeof state !== 'string' || ![
      'TASK_STATE_SUBMITTED', 'TASK_STATE_WORKING', 'TASK_STATE_COMPLETED', 'TASK_STATE_FAILED',
      'TASK_STATE_CANCELED', 'TASK_STATE_INPUT_REQUIRED', 'TASK_STATE_AUTH_REQUIRED', 'TASK_STATE_REJECTED',
    ].includes(state)
      || 'namespace' in listed || 'tenant' in listed
      || (listed.metadata && typeof listed.metadata === 'object' && !Array.isArray(listed.metadata) && 'acteon_governed_execution' in listed.metadata)) throw new Error('Agent service task projection is malformed')
    return listed as unknown as ListedServiceTask
  })
  return Object.freeze({ tasks: Object.freeze(tasks), nextPageToken: page.nextPageToken, pageSize: 25, totalSize: page.totalSize })
}
