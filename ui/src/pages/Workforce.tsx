import { useState } from 'react'
import { useWorkforce, useWorkforceChange } from '../api/hooks/useWorkforce'
import { PageHeader } from '../components/layout/PageHeader'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Button } from '../components/ui/Button'
import { Badge } from '../components/ui/Badge'
import { Modal } from '../components/ui/Modal'
import type { GovernanceChangeReceipt, WorkforceChange, WorkforceChangeRequest, WorkforceScopeView } from '../types'
import { WorkforceEnrollment } from './workforce/WorkforceEnrollment'
import { WorkforceMandateForm } from './workforce/WorkforceMandateForm'
import styles from './Governance.module.css'

export type WorkforceReview = (label: string, change: WorkforceChange) => void
export function Workforce() {
  const [namespace, setNamespace] = useState('')
  const [tenant, setTenant] = useState('')
  const [scope, setScope] = useState({ namespace: '', tenant: '' })
  const query = useWorkforce(scope.namespace, scope.tenant)
  const mutation = useWorkforceChange()
  const [pending, setPending] = useState<{ label: string; change: WorkforceChange } | null>(null)
  const [reason, setReason] = useState('')
  const [frozen, setFrozen] = useState<WorkforceChangeRequest | null>(null)
  const [receipt, setReceipt] = useState<GovernanceChangeReceipt | null>(null)
  const [error, setError] = useState('')
  const view = query.data
  const review: WorkforceReview = (label, change) => {
    setPending({ label, change }); setFrozen(null); setReason(''); setError('')
  }
  const apply = async () => {
    if (!pending || !reason.trim()) return
    const request = frozen ?? { ...scope, change_id: crypto.randomUUID(), change: pending.change, reason: reason.trim() }
    setFrozen(request)
    try {
      setReceipt(await mutation.mutateAsync(request)); setPending(null); setFrozen(null); setError('')
    } catch (e) { setError((e as Error).message) }
  }
  return <div className={styles.content}>
    <PageHeader title="Workforce" subtitle="Organize people and agents. Issue explicit mandates for the work they represent." />
    <form className={styles.scope} onSubmit={e => { e.preventDefault(); if (!mutation.isPending && !pending) { setScope({ namespace: namespace.trim(), tenant: tenant.trim() }); setReceipt(null); setError('') } }}>
      <Input label="Namespace" value={namespace} onChange={e => setNamespace(e.target.value)} required disabled={!!pending} />
      <Input label="Tenant" value={tenant} onChange={e => setTenant(e.target.value)} required disabled={!!pending} />
      <Button loading={query.isFetching} disabled={mutation.isPending || !!pending}>Inspect workforce</Button>
    </form>
    {query.error && <p className={styles.error} role="alert">{query.error.message}</p>}
    {receipt && <div className={styles.card} role="status"><strong>Workforce change recorded</strong><p className={styles.muted}>{receipt.reason} · generation {receipt.generation}</p></div>}
    {view && <>
      <WorkforceRoster view={view} review={review} disabled={mutation.isPending || !!pending} />
      {view.management.can_manage_roster && <WorkforceEnrollment key={`${scope.namespace}/${scope.tenant}`} view={view} review={review} disabled={mutation.isPending || !!pending} />}
      <section className={styles.card}><h2>Representation mandates</h2><p className={styles.muted}>Each mandate retains the actual actor and represented party. Current relationships and permits are checked before each new effect.</p>
        {!view.mandates.length && <p className={styles.muted}>No mandates in this management scope.</p>}
        {view.mandates.map(record => <div className={styles.row} key={record.value.id}><div className={styles.identity}><strong>{record.value.id}</strong><p className={styles.muted}>{record.value.actor.id} → {record.value.represented.kind === 'team' ? record.value.represented.team.id : record.value.represented.principal.id} · {record.value.job_class} · revision {record.value.revision}</p><p className={styles.muted}>{record.value.dependencies.length} explicit dependencies · {record.value.limits.max_units} calls per root</p></div><div className={styles.actions}><Badge variant={record.revoked ? 'error' : 'success'}>{record.revoked ? 'Revoked' : 'Issued'}</Badge><Button size="sm" variant="danger" disabled={!!pending || record.revoked || !view.management.can_issue_mandates} onClick={() => review(`Revoke ${record.value.id}`, { kind: 'revoke_mandate', id: record.value.id, expected_revision: record.value.revision })}>Revoke mandate</Button></div></div>)}
      </section>
      {view.management.can_issue_mandates && <WorkforceMandateForm key={`mandate/${scope.namespace}/${scope.tenant}`} view={view} review={review} disabled={mutation.isPending || !!pending} />}
      {view.management.can_issue_permits && <WorkforcePermitForm key={`permit/${scope.namespace}/${scope.tenant}`} view={view} review={review} disabled={mutation.isPending || !!pending} />}
    </>}
    <Modal open={!!pending} title={pending?.label ?? 'Review workforce change'} onClose={() => { if (!mutation.isPending) { setPending(null); setFrozen(null); setError('') } }} footer={<><Button variant="secondary" disabled={mutation.isPending} onClick={() => { setPending(null); setFrozen(null); setError('') }}>Cancel</Button><Button loading={mutation.isPending} disabled={!reason.trim() || mutation.isPending} onClick={() => void apply()}>{frozen ? 'Retry same change' : 'Apply change'}</Button></>}>
      <p className={styles.muted}>This change uses your current workforce management bounds. Removing a relationship or revoking a mandate blocks dependent new effects; already started effects may finish.</p>
      <Input label="Reason" value={reason} disabled={!!frozen} onChange={e => setReason(e.target.value)} />
      {error && <p className={styles.error} role="alert">{error}</p>}
    </Modal>
  </div>
}
function WorkforceRoster({ view, review, disabled }: { view: WorkforceScopeView; review: WorkforceReview; disabled: boolean }) {
  const canManage = view.management.can_manage_roster && !disabled
  return <>
    <section className={styles.card}><h2>Teams</h2>{!view.teams.length && <p className={styles.muted}>No teams enrolled.</p>}
      {view.teams.map(r => <div className={styles.row} key={JSON.stringify(r.value.team)}><div className={styles.identity}><strong>{r.value.name}</strong><p className={styles.muted}>{r.value.team.domain} / {r.value.team.id} · revision {r.value.revision}</p></div><div className={styles.actions}><Badge variant={r.revoked ? 'error' : 'success'}>{r.revoked ? 'Disbanded' : 'Active'}</Badge><Button size="sm" variant="danger" disabled={!canManage || r.revoked} onClick={() => review(`Disband ${r.value.name}`, { kind: 'disband_team', team: r.value.team, expected_revision: r.value.revision })}>Disband team</Button></div></div>)}
    </section>
    <section className={styles.card}><h2>Human membership</h2>{!view.memberships.length && <p className={styles.muted}>No memberships recorded.</p>}
      {view.memberships.map(r => <div className={styles.row} key={r.value.id}><div className={styles.identity}><strong>{r.value.human.id}</strong><p className={styles.muted}>{r.value.team.id} · {r.value.roles.join(', ')} · revision {r.value.revision}</p></div><Button size="sm" variant="danger" disabled={!canManage || r.revoked} onClick={() => review(`Remove ${r.value.human.id} from ${r.value.team.id}`, { kind: 'remove_membership', id: r.value.id, expected_revision: r.value.revision })}>{r.revoked ? 'Removed' : 'Remove membership'}</Button></div>)}
    </section>
    <section className={styles.card}><h2>Agent ownership</h2><p className={styles.muted}>Ownership records stewardship. It grants no tools or execution rights.</p>
      {view.ownership.map(r => <div className={styles.row} key={r.value.agent.id}><div className={styles.identity}><strong>{r.value.agent.id}</strong><p className={styles.muted}>Owner: {r.value.owner.kind === 'human' ? r.value.owner.principal.id : r.value.owner.team.id} · revision {r.value.revision}</p></div></div>)}
    </section>
    <section className={styles.card}><h2>Duty assignments</h2>{!view.assignments.length && <p className={styles.muted}>No assignments recorded.</p>}
      {view.assignments.map(r => <div className={styles.row} key={r.value.id}><div className={styles.identity}><strong>{r.value.agent.id}</strong><p className={styles.muted}>{r.value.team.id} · {r.value.job_classes.join(', ')} · revision {r.value.revision}</p></div><Button size="sm" variant="danger" disabled={!canManage || r.revoked} onClick={() => review(`Remove ${r.value.id}`, { kind: 'remove_assignment', id: r.value.id, expected_revision: r.value.revision })}>{r.revoked ? 'Removed' : 'Remove assignment'}</Button></div>)}
    </section>
  </>
}
function WorkforcePermitForm({ view, review, disabled }: { view: WorkforceScopeView; review: WorkforceReview; disabled: boolean }) {
  const [mandateId, setMandateId] = useState('')
  const [permitId, setPermitId] = useState('')
  const [error, setError] = useState('')
  const submit = () => {
    const mandate = view.mandates.find(m => m.value.id === mandateId && !m.revoked)?.value
    if (!mandate || !permitId.trim()) { setError('Choose a current mandate and a new permit ID.'); return }
    const key = (e: WorkforceScopeView['routes'][number]['effect']) => JSON.stringify([e.operation, e.resources.map(r => JSON.stringify([r.kind, r.namespace, r.tenant, r.id])).sort()])
    const routes = view.routes.filter(r => mandate.effects.some(e => key(e) === key(r.effect))).map(r => r.route)
    if (!routes.length) { setError('This mandate has no currently qualified route.'); return }
    const limits = {
      max_units: Math.min(mandate.limits.max_units, view.management.limits.max_units),
      max_concurrent: Math.min(mandate.limits.max_concurrent, view.management.limits.max_concurrent),
      deadline_ms: Math.min(mandate.limits.deadline_ms, view.management.limits.deadline_ms),
    }
    setError(''); review(`Issue ${permitId.trim()}`, { kind: 'publish_represented_permit',
      permit: { id: permitId.trim(), revision: 1, subject: mandate.actor, routes, valid_from_ms: Math.max(mandate.valid_from_ms, view.management.valid_from_ms), limits },
      mandate: { id: mandate.id, accepted_revision: mandate.revision } })
  }
  return <section className={styles.card}><h2>Issue a represented permit</h2><p className={styles.muted}>The new permit uses the selected mandate's actor and qualified effects. Its budgets and validity fit both the mandate and your management bounds. Its mandate binding is committed with the permit.</p><div className={styles.grid}>
    <Select label="Mandate" value={mandateId} onChange={e => setMandateId(e.target.value)} placeholder="Choose a mandate" options={view.mandates.filter(m => !m.revoked).map(m => ({ value: m.value.id, label: m.value.id }))} />
    <Input label="Permit ID" value={permitId} onChange={e => setPermitId(e.target.value)} />
    </div>{error && <p role="alert" className={styles.error}>{error}</p>}<Button disabled={disabled} onClick={submit}>Review represented permit</Button></section>
}
