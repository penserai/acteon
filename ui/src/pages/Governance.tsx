import { useState } from 'react'
import { useGovernance, useGovernanceIntervention, usePublishGovernancePermit } from '../api/hooks/useGovernance'
import { PageHeader } from '../components/layout/PageHeader'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Button } from '../components/ui/Button'
import { Badge } from '../components/ui/Badge'
import { Modal } from '../components/ui/Modal'
import type { GovernanceChangeReceipt, GovernanceIntervention, GovernanceResource } from '../types'
import { ProviderHistory } from './ProviderHistory'
import { RegistryManagement } from './governance/RegistryManagement'
import styles from './Governance.module.css'

const resourceKey = (resource: GovernanceResource) => JSON.stringify(resource)
export function Governance() {
  const [namespace, setNamespace] = useState('')
  const [tenant, setTenant] = useState('')
  const [scope, setScope] = useState({ namespace: '', tenant: '' })
  const query = useGovernance(scope.namespace, scope.tenant)
  const intervention = useGovernanceIntervention()
  const publication = usePublishGovernancePermit()
  const [pending, setPending] = useState<{ label: string; change: GovernanceIntervention; id: string; namespace: string; tenant: string } | null>(null)
  const [reason, setReason] = useState('')
  const [receipt, setReceipt] = useState<GovernanceChangeReceipt | null>(null)
  const [error, setError] = useState('')
  const [permitId, setPermitId] = useState('')
  const [subject, setSubject] = useState('')
  const [route, setRoute] = useState('')
  const [units, setUnits] = useState('1')
  const [concurrent, setConcurrent] = useState('1')
  const [deadline, setDeadline] = useState('')
  const [issueReason, setIssueReason] = useState('')
  const [issueId, setIssueId] = useState<string | null>(null)
  const [registryLocked, setRegistryLocked] = useState(false)
  const view = query.isError ? undefined : query.data
  const busy = intervention.isPending || publication.isPending || registryLocked
  const resources = [...new Map(view?.routes.flatMap(r => r.effect.resources).map(r => [resourceKey(r), r]) ?? []).values()]
  const begin = (label: string, change: GovernanceIntervention) => {
    setPending({ label, change, id: crypto.randomUUID(), ...scope }); setReason(''); setError('')
  }
  const confirm = async () => {
    if (!pending || !reason.trim()) return
    try {
      const result = await intervention.mutateAsync({ namespace: pending.namespace, tenant: pending.tenant, change_id: pending.id, change: pending.change, reason: reason.trim() })
      setReceipt(result); setPending(null); setError('')
    } catch (e) { setError((e as Error).message) }
  }
  const issue = async () => {
    if (!view) return
    const chosenSubject = view.management.subjects.find(s => s.id === subject)
    const chosenRoute = view.routes.find(r => resourceKey(r.effect.resources[0]) + r.route.provider + r.route.action_type === route)
    const caps = { max_units: Number(units), max_concurrent: Number(concurrent), deadline_ms: Date.parse(deadline) }
    if (!chosenSubject || !chosenRoute || !permitId.trim() || !issueReason.trim()
      || !Object.values(caps).every(Number.isSafeInteger) || caps.max_units < 1 || caps.max_concurrent < 1 || caps.deadline_ms <= Date.now()) {
      setError('Choose a subject and route, positive limits, a future deadline, and a reason.'); return
    }
    const id = issueId ?? crypto.randomUUID(); setIssueId(id)
    try {
      const result = await publication.mutateAsync({ ...scope, change_id: id, expected_revision: 0,
        permit: { id: permitId.trim(), revision: 1, subject: chosenSubject, routes: [chosenRoute.route],
          valid_from_ms: view.management.valid_from_ms, limits: caps }, reason: issueReason.trim() })
      setReceipt(result); setError(''); setIssueId(null); setPermitId('')
    } catch (e) { setError((e as Error).message) }
  }
  return <div className={styles.content}>
    <PageHeader title="Governance" subtitle="Control execution authority and inspect retained evidence." />
    <form className={styles.scope} onSubmit={e => { e.preventDefault(); if (!busy) { setScope({ namespace: namespace.trim(), tenant: tenant.trim() }); setReceipt(null); setPending(null); setError(''); setIssueId(null) } }}>
      <Input label="Namespace" value={namespace} onChange={e => setNamespace(e.target.value)} required />
      <Input label="Tenant" value={tenant} onChange={e => setTenant(e.target.value)} required />
      <Button loading={query.isFetching} disabled={busy}>Inspect scope</Button>
    </form>
    {query.error && <p className={styles.error} role="alert">{query.error.message}</p>}
    {error && <p className={styles.error} role="alert">{error}</p>}
    {receipt && <div className={styles.card} role="status"><strong>Control recorded</strong><p className={styles.muted}>Generation {receipt.generation} · {receipt.reason}</p><p className={styles.muted}>New execution checks use this authority. Already started effects may still finish.</p></div>}
    {view && <>
      {view.management.can_read_history && <ProviderHistory key={JSON.stringify([view.namespace, view.tenant])} namespace={view.namespace} tenant={view.tenant} />}
      <RegistryManagement key={JSON.stringify([view.namespace, view.tenant])} namespace={view.namespace} tenant={view.tenant} canIntervene={view.management.can_intervene} onLockChange={setRegistryLocked} />
      <section className={styles.card}><h2>Governed routes</h2>
        {view.routes.map(r => <div className={styles.row} key={r.route.provider + r.route.action_type}><div className={styles.identity}><strong>{r.route.provider} / {r.route.action_type}</strong><p className={styles.muted}>{r.effect.operation}</p></div><Badge variant={r.closed ? 'warning' : 'success'}>{r.closed ? 'Closed' : 'Open'}</Badge></div>)}
      </section>
      <section className={styles.card}><h2>Resources</h2><p className={styles.muted}>A closure refuses new effects requiring that exact resource. Shared resources can affect several routes.</p>
        {resources.map(resource => {
          const closed = view.closed_resources.some(r => resourceKey(r) === resourceKey(resource))
          return <div className={styles.row} key={resourceKey(resource)}><div className={styles.identity}><strong>{resource.kind}</strong><p className={styles.muted}>{resource.id}</p></div><Button variant={closed ? 'secondary' : 'danger'} size="sm" disabled={busy || !view.management.can_intervene}
            onClick={() => begin(`${closed ? 'Reopen' : 'Close'} ${resource.kind}`, { kind: closed ? 'reopen_resource' : 'close_resource', resource })}>{closed ? 'Reopen' : 'Close'}</Button></div>
        })}
      </section>
      <section className={styles.card}><h2>Permits</h2>
        {view.permits.map(permit => <div className={styles.row} key={permit.id}><div className={styles.identity}><strong>{permit.id}</strong><p className={styles.muted}>{permit.subject.id} · revision {permit.revision} · {permit.limits.max_units} calls per root</p></div><div className={styles.actions}><Badge variant={permit.revoked ? 'error' : 'success'}>{permit.revoked ? 'Revoked' : 'Issued'}</Badge><Button variant="danger" size="sm" disabled={busy || permit.revoked || !view.management.can_intervene} onClick={() => begin(`Revoke ${permit.id}`, { kind: 'revoke_permit', permit_id: permit.id, expected_revision: permit.revision })}>Revoke</Button></div></div>)}
      </section>
      {view.management.can_issue_permits && <section className={styles.card}><h2>Issue a permit</h2><p className={styles.muted}>A new permit ID starts at revision 1. Limits must fit your management policy.</p>
        <div className={styles.grid}>
          <Input label="Permit ID" value={permitId} onChange={e => { setPermitId(e.target.value); setIssueId(null) }} />
          <Select label="Subject" value={subject} options={view.management.subjects.map(s => ({ value: s.id, label: s.id }))} placeholder="Choose a subject" onChange={e => { setSubject(e.target.value); setIssueId(null) }} />
          <Select label="Route" value={route} options={view.routes.map(r => ({ value: resourceKey(r.effect.resources[0]) + r.route.provider + r.route.action_type, label: `${r.route.provider} / ${r.route.action_type}` }))} placeholder="Choose a route" onChange={e => { setRoute(e.target.value); setIssueId(null) }} />
          <Input label="Deadline" type="datetime-local" value={deadline} onChange={e => { setDeadline(e.target.value); setIssueId(null) }} />
          <Input label="Maximum calls per root" type="number" min="1" value={units} onChange={e => { setUnits(e.target.value); setIssueId(null) }} />
          <Input label="Maximum concurrent calls" type="number" min="1" value={concurrent} onChange={e => { setConcurrent(e.target.value); setIssueId(null) }} />
        </div>
        <Input label="Issuance reason" value={issueReason} onChange={e => { setIssueReason(e.target.value); setIssueId(null) }} />
        <Button onClick={() => void issue()} disabled={busy} loading={publication.isPending}>Issue permit</Button>
      </section>}
    </>}
    <Modal open={!!pending} onClose={() => { if (!busy) setPending(null) }} title={pending?.label ?? 'Governance change'} footer={<><Button variant="secondary" disabled={busy} onClick={() => setPending(null)}>Cancel</Button><Button variant="danger" loading={intervention.isPending} disabled={!reason.trim() || busy} onClick={() => void confirm()}>Apply change</Button></>}>
      <p className={styles.muted}>The current management policy is checked before this change is recorded. Closing or revoking authority does not undo completed effects.</p>
      <Input label="Reason" value={reason} onChange={e => setReason(e.target.value)} />
      {error && <p className={styles.error} role="alert">{error}</p>}
    </Modal>
  </div>
}
