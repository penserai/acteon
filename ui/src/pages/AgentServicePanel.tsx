import { useState } from 'react'
import { sendServiceMessage, observeServiceTask, stopServiceTask, type ServiceProviderAbort, type ServiceReceipt } from '../api/agentServices'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Badge } from '../components/ui/Badge'

export function AgentServicePanel({ namespace, tenant, agent }: { namespace: string; tenant: string; agent: string }) {
  const [messageId, setMessageId] = useState(() => crypto.randomUUID())
  const [text, setText] = useState('')
  const [attempted, setAttempted] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [receipts, setReceipts] = useState<(ServiceReceipt & { futureStartsBlocked?: boolean; providerAbort?: ServiceProviderAbort })[]>([])
  async function send() {
    setBusy(true); setError(''); setAttempted(true)
    try {
      const accepted = await sendServiceMessage(namespace, tenant, agent, messageId, text)
      setReceipts(current => [accepted, ...current.filter(receipt => receipt.taskId !== accepted.taskId)])
    } catch (cause) { setError((cause as Error).message) }
    finally { setBusy(false) }
  }
  async function refresh(receipt: ServiceReceipt) {
    setBusy(true); setError('')
    try {
      const task = await observeServiceTask(receipt)
      setReceipts(current => current.map(item => item.taskId === receipt.taskId ? { ...item, task } : item))
    } catch (cause) { setError((cause as Error).message) }
    finally { setBusy(false) }
  }
  async function stop(receipt: ServiceReceipt) {
    setBusy(true); setError('')
    try {
      const stopped = await stopServiceTask(receipt)
      setReceipts(current => current.map(item => item.taskId === receipt.taskId ? { ...item, task: stopped.task, futureStartsBlocked: true, providerAbort: stopped.providerAbort } : item))
    } catch (cause) { setError(`Stop acknowledgement for task ${receipt.taskId} unavailable. Retry this task’s stop control. ${(cause as Error).message}`) }
    finally { setBusy(false) }
  }
  return <section className="space-y-4" aria-label="Governed agent tasks">
    <h2 className="text-lg font-semibold">Governed tasks</h2>
    <p>Send work to this agent’s configured service using your current identity and permits. Accepted tasks may still be waiting to execute.</p>
    <p className="text-sm">Task receipts stay in this view while it is open. Leaving this tab or refreshing the page clears the local view.</p>
    <Input label="Message" value={text} disabled={busy || attempted} onChange={event => setText(event.target.value)} placeholder="Describe the requested operation" />
    <p className="text-xs font-mono break-all">Request {messageId}</p>
    <div className="flex gap-2">
      <Button disabled={busy || !text.trim()} onClick={() => void send()}>{attempted ? 'Retry same request' : 'Send request'}</Button>
      <Button variant="secondary" disabled={busy} onClick={() => { setMessageId(crypto.randomUUID()); setText(''); setAttempted(false); setError('') }}>New request</Button>
    </div>
    {error && <p role="alert">{error}</p>}
    {receipts.length === 0 && <p>No accepted tasks in this session. This agent must have a configured governed service and you must have invocation authority.</p>}
    {receipts.map(receipt => <article key={receipt.taskId} className="rounded-lg border p-4 space-y-2">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <code className="min-w-0 break-all">{receipt.taskId}</code><Badge variant={receipt.task.status.state === 'completed' ? 'success' : 'neutral'}>{receipt.task.status.state}</Badge>
      </div>
      <div className="flex flex-wrap gap-2">
        <Button variant="secondary" disabled={busy} onClick={() => void refresh(receipt)}>Refresh task</Button>
        <Button variant="secondary" disabled={busy || receipt.futureStartsBlocked === true} onClick={() => void stop(receipt)}>Stop future starts</Button>
      </div>
      {receipt.futureStartsBlocked && <p className="text-sm">{receipt.providerAbort?.state === 'reconciled'
        ? 'Future starts stopped. Qualified provider finality reconciled the registered attempt.'
        : receipt.providerAbort?.state === 'uncertain'
          ? `Future starts stopped. Provider abort outcome is unresolved for attempt ${receipt.providerAbort.attemptId}; retained capacity requires reconciliation.`
          : receipt.providerAbort?.state === 'restricted_only'
            ? 'Future starts stopped. This provider has no qualified abort capability; a delivered operation may still complete.'
            : 'Future starts stopped. No registered provider attempt required an abort.'}</p>}
      {!!receipt.task.artifacts?.length && <pre className="overflow-auto text-xs">{JSON.stringify(receipt.task.artifacts, null, 2)}</pre>}
    </article>)}
  </section>
}
