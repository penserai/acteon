import { useEffect, useRef, useState } from 'react'
import { useGovernanceRegistryMutation, useGovernanceRegistryProjection } from '../../api/hooks/useGovernance'
import { Badge } from '../../components/ui/Badge'
import { Button } from '../../components/ui/Button'
import { Input } from '../../components/ui/Input'
import { Select } from '../../components/ui/Select'
import type { GovernanceRegistryMutationReceipt, GovernanceRegistryMutationRequest, GovernanceRegistryProjection, GovernanceRegistryProjectionView } from '../../types'
import styles from '../Governance.module.css'

type Props = { namespace: string; tenant: string; canIntervene: boolean; onLockChange: (locked: boolean) => void }
const objectValue = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value)
const journalKey = (namespace: string, tenant: string) => `acteon-registry-intent:${namespace}:${tenant}`
function loadJournal(namespace: string, tenant: string): GovernanceRegistryMutationRequest | null {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(journalKey(namespace, tenant)) ?? 'null')
    if (!objectValue(parsed) || parsed.namespace !== namespace || parsed.tenant !== tenant
      || typeof parsed.agent_id !== 'string' || !parsed.agent_id || typeof parsed.change_id !== 'string' || !parsed.change_id
      || (parsed.projection !== 'agent' && parsed.projection !== 'card')
      || !Number.isSafeInteger(parsed.expected_registry_revision) || (parsed.expected_registry_revision as number) < 0
      || (parsed.expected_projection_version !== null && (!Number.isSafeInteger(parsed.expected_projection_version) || (parsed.expected_projection_version as number) <= 0))
      || (parsed.value !== null && !objectValue(parsed.value)) || typeof parsed.reason !== 'string' || !parsed.reason) return null
    return parsed as unknown as GovernanceRegistryMutationRequest
  } catch { return null }
}
function saveJournal(request: GovernanceRegistryMutationRequest): boolean {
  try { localStorage.setItem(journalKey(request.namespace, request.tenant), JSON.stringify(request)); return true }
  catch { return false }
}
function clearJournal(namespace: string, tenant: string) {
  try { localStorage.removeItem(journalKey(namespace, tenant)) } catch { /* unavailable storage is already fail-closed on review */ }
}

export function RegistryManagement({ namespace, tenant, canIntervene, onLockChange }: Props) {
  const inspection = useGovernanceRegistryProjection()
  const mutation = useGovernanceRegistryMutation()
  const [recovered] = useState(() => loadJournal(namespace, tenant))
  const [agentId, setAgentId] = useState(recovered?.agent_id ?? '')
  const [projection, setProjection] = useState<GovernanceRegistryProjection>(recovered?.projection ?? 'card')
  const [view, setView] = useState<GovernanceRegistryProjectionView | null>(null)
  const [editor, setEditor] = useState('{}')
  const [reason, setReason] = useState(recovered?.reason ?? '')
  const [pending, setPending] = useState<GovernanceRegistryMutationRequest | null>(recovered)
  const [attempted, setAttempted] = useState(!!recovered)
  const [restored, setRestored] = useState(!!recovered)
  const [receipt, setReceipt] = useState<GovernanceRegistryMutationReceipt | null>(null)
  const [error, setError] = useState('')
  const sending = useRef(false)
  const busy = inspection.isPending || mutation.isPending

  useEffect(() => {
    onLockChange(busy || !!pending)
    return () => onLockChange(false)
  }, [busy, onLockChange, pending])

  const resetObserved = () => { setView(null); setPending(null); setAttempted(false); setReceipt(null); setError('') }
  const inspect = async () => {
    const id = agentId.trim()
    if (!id || !canIntervene) return
    try {
      const result = await inspection.mutateAsync({ namespace, tenant, agentId: id, projection })
      setView(result); setEditor(JSON.stringify(result.value ?? {}, null, 2)); setReason('')
      setPending(null); setAttempted(false); setReceipt(null); setError('')
    } catch (e) { setView(null); setError((e as Error).message) }
  }
  const review = (remove: boolean) => {
    if (!view || !reason.trim()) return
    let value: Record<string, unknown> | null = null
    if (!remove) {
      try {
        const parsed: unknown = JSON.parse(editor)
        if (!objectValue(parsed)) throw new Error('Projection JSON must be an object')
        value = parsed
      } catch (e) { setError(`Invalid projection JSON: ${(e as Error).message}`); return }
    }
    const request: GovernanceRegistryMutationRequest = { namespace: view.namespace, tenant: view.tenant, agent_id: view.agent_id,
      change_id: crypto.randomUUID(), expected_registry_revision: view.registry_revision,
      projection: view.projection, expected_projection_version: view.version, value, reason: reason.trim() }
    if (!saveJournal(request)) { setError('This browser could not retain the reviewed request. No registry change was sent.'); return }
    setPending(request); setAttempted(false); setRestored(false); setReceipt(null); setError('')
  }
  const send = async () => {
    if (!pending || sending.current) return
    sending.current = true
    setAttempted(true)
    try {
      const result = await mutation.mutateAsync(pending)
      clearJournal(namespace, tenant)
      setReceipt(result); setPending(null); setRestored(false); setView(null); setReason(''); setError('')
    } catch (e) { setError((e as Error).message) }
    finally { sending.current = false }
  }
  const discard = () => {
    clearJournal(namespace, tenant)
    setPending(null); setAttempted(false); setRestored(false); setView(null); setError('')
  }
  const qualification = !view || view.registry_revision === 0 ? 'Unqualified' : view.qualification_retired ? 'Retired' : 'Qualified'

  return <section className={styles.card} aria-labelledby="registry-heading">
    <h2 id="registry-heading">Registry metadata</h2>
    <p className={styles.muted}>Inspect and replace one agent or card record under exact current agent-management authority. Every write retires the current qualification; metadata publication does not restore permission to execute.</p>
    <div className={styles.grid}>
      <Input label="Registry agent ID" value={agentId} disabled={busy || !!pending} onChange={e => { setAgentId(e.target.value); resetObserved() }} />
      <Select label="Registry projection" value={projection} disabled={busy || !!pending} options={[{ value: 'agent', label: 'Agent record' }, { value: 'card', label: 'Agent card' }]} onChange={e => { setProjection(e.target.value as GovernanceRegistryProjection); resetObserved() }} />
    </div>
    <div className={styles.actions}>
      <Button variant="secondary" loading={inspection.isPending} disabled={busy || !!pending || !agentId.trim() || !canIntervene} onClick={() => void inspect()}>Inspect registry record</Button>
    </div>
    {!canIntervene && <p className={styles.muted}>This management view cannot intervene. Registry inspection and mutation are unavailable.</p>}
    {error && <p className={styles.error} role="alert">{error}</p>}
    {restored && pending && <p className={styles.muted} role="status">Recovered the exact reviewed request from this browser. Retry it unchanged or discard it and inspect again.</p>}
    {receipt && <div className={styles.registryReview} role="status"><strong>Registry change applied</strong><p className={styles.muted}>Change {receipt.change_id} · recorded by {receipt.actor}. Inspect the record again before forming another intent; qualification remains retired until independently approved.</p></div>}
    {view && <>
      <div className={styles.registryMeta}>
        <Badge variant={qualification === 'Qualified' ? 'success' : qualification === 'Retired' ? 'warning' : 'neutral'}>{qualification}</Badge>
        <span className={styles.muted}>Registry revision {view.registry_revision} · backend version {view.version ?? 'absent'} · {view.projection}</span>
      </div>
      <label htmlFor="registry-projection-json" className={styles.muted}>Projection JSON</label>
      <textarea id="registry-projection-json" className={styles.registryTextarea} value={editor} disabled={busy || !!pending || !canIntervene} onChange={e => setEditor(e.target.value)} spellCheck={false} />
      <Input label="Registry change reason" value={reason} disabled={busy || !!pending || !canIntervene} onChange={e => setReason(e.target.value)} />
      <div className={styles.actions}>
        <Button disabled={busy || !!pending || !reason.trim() || !canIntervene} onClick={() => review(false)}>Review metadata update</Button>
        <Button variant="danger" disabled={busy || !!pending || !view.value || !reason.trim() || !canIntervene} onClick={() => review(true)}>Review metadata removal</Button>
      </div>
    </>}
    {pending && <div className={styles.registryReview}>
      <strong>Reviewed {pending.value === null ? 'removal' : 'update'}</strong>
      <p className={styles.muted}>Change {pending.change_id} uses registry revision {pending.expected_registry_revision} and backend version {pending.expected_projection_version ?? 'absent'}. A failed or unavailable acknowledgement leaves this exact request locked for explicit recovery.</p>
      <pre className={styles.registryJson}>{JSON.stringify(pending.value, null, 2)}</pre>
      <div className={styles.actions}>
        <Button loading={mutation.isPending} disabled={busy} onClick={() => void send()}>{attempted ? 'Retry same registry change' : 'Apply reviewed registry change'}</Button>
        <Button variant="secondary" disabled={busy} onClick={discard}>Discard and inspect again</Button>
      </div>
    </div>}
  </section>
}
