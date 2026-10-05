import { useState } from 'react'
import { Input } from '../../components/ui/Input'
import { Select } from '../../components/ui/Select'
import { Button } from '../../components/ui/Button'
import type { RepresentedParty, WorkforceDependency, WorkforceScopeView } from '../../types'
import type { WorkforceReview } from '../Workforce'
import styles from '../Governance.module.css'

export function WorkforceMandateForm({ view, review, disabled }: { view: WorkforceScopeView; review: WorkforceReview; disabled: boolean }) {
  const [id, setId] = useState('')
  const [actorId, setActorId] = useState('')
  const [representedKind, setRepresentedKind] = useState<'team' | 'human'>('team')
  const [partyId, setPartyId] = useState('')
  const [jobClass, setJobClass] = useState('')
  const [routeKey, setRouteKey] = useState('')
  const [initiators, setInitiators] = useState<string[]>([])
  const [dependencies, setDependencies] = useState<string[]>([])
  const [units, setUnits] = useState('1')
  const [concurrent, setConcurrent] = useState('1')
  const [deadline, setDeadline] = useState('')
  const [error, setError] = useState('')
  const choices: { key: string; label: string; dependency: WorkforceDependency }[] = [
    ...view.memberships.filter(r => !r.revoked).map(r => ({ key: `membership:${r.value.id}`, label: `Membership: ${r.value.human.id} in ${r.value.team.id}`, dependency: { kind: 'membership' as const, reference: { id: r.value.id, accepted_revision: r.value.revision } } })),
    ...view.assignments.filter(r => !r.revoked && r.value.agent.id === actorId).map(r => ({ key: `assignment:${r.value.id}`, label: `Assignment: ${r.value.id}`, dependency: { kind: 'assignment' as const, reference: { id: r.value.id, accepted_revision: r.value.revision } } })),
  ]
  const submit = (now: number) => {
    const actor = view.management.principals.find(p => p.id === actorId)
    const route = view.routes.find(r => JSON.stringify(r.route) === routeKey && r.route.action_type === jobClass)
    const limits = { max_units: Number(units), max_concurrent: Number(concurrent), deadline_ms: Date.parse(deadline) }
    const selectedInitiators = view.management.principals.filter(p => initiators.includes(p.id))
    let represented: RepresentedParty
    if (representedKind === 'team') {
      const team = view.teams.find(r => !r.revoked && JSON.stringify(r.value.team) === partyId)?.value.team
      if (!team) { setError('Choose an active represented team.'); return }
      represented = { kind: 'team', team }
    } else {
      const principal = view.management.principals.find(p => p.id === partyId && p.kind === 'human')
      if (!principal) { setError('Choose the represented human.'); return }
      represented = { kind: 'human', principal }
    }
    if (!id.trim() || !actor || !route || !selectedInitiators.length || !Object.values(limits).every(Number.isSafeInteger)
      || limits.max_units < 1 || limits.max_concurrent < 1 || limits.deadline_ms <= now
      || limits.max_units > view.management.limits.max_units || limits.max_concurrent > view.management.limits.max_concurrent
      || limits.deadline_ms > view.management.limits.deadline_ms) { setError('Choose the actor, route, requesters and positive limits within your management bounds.'); return }
    const existing = view.mandates.find(r => r.value.id === id.trim())
    if (existing?.revoked) { setError('Use a new ID for a revoked mandate.'); return }
    const ownership = view.ownership.find(r => r.value.agent.id === actor.id && !r.revoked)?.value
    if (actor.kind === 'agent' && !ownership) { setError('Enroll this agent’s ownership before issuing a mandate.'); return }
    const selectedDependencies = choices.filter(c => dependencies.includes(c.key)).map(c => c.dependency)
    if (selectedDependencies.length !== dependencies.length) { setError('A selected dependency changed. Review the current relationships.'); return }
    setError(''); review(`Issue ${id.trim()} for ${actor.id}`, { kind: 'put_mandate', mandate: {
      id: id.trim(), revision: (existing?.value.revision ?? 0) + 1, actor, represented, job_class: jobClass,
      eligible_initiators: selectedInitiators, ownership: ownership ? { id: actor.id, accepted_revision: ownership.revision } : null,
      dependencies: selectedDependencies, routes: [route.route], valid_from_ms: view.management.valid_from_ms, limits } })
  }
  const toggle = (values: string[], key: string) => values.includes(key) ? values.filter(v => v !== key) : [...values, key]
  return <section className={styles.card}><h2>Issue a representation mandate</h2><p className={styles.muted}>Pin ownership and choose every required membership or assignment explicitly. Standing team work need not depend on its creator’s membership.</p><div className={styles.grid}>
    <Input label="Mandate ID" value={id} onChange={e => setId(e.target.value)} />
    <Select label="Actual actor" value={actorId} onChange={e => { setActorId(e.target.value); setDependencies([]); setInitiators([e.target.value]) }} placeholder="Choose an actor" options={view.management.principals.map(p => ({ value: p.id, label: `${p.id} (${p.kind})` }))} />
    <Select label="Represents" value={representedKind} onChange={e => { setRepresentedKind(e.target.value as 'team' | 'human'); setPartyId('') }} options={[{ value: 'team', label: 'Team' }, { value: 'human', label: 'Human' }]} />
    <Select label="Represented party" value={partyId} onChange={e => setPartyId(e.target.value)} placeholder="Choose a party" options={representedKind === 'team' ? view.teams.filter(r => !r.revoked).map(r => ({ value: JSON.stringify(r.value.team), label: r.value.name })) : view.management.principals.filter(p => p.kind === 'human').map(p => ({ value: p.id, label: p.id }))} />
    <Select label="Job class" value={jobClass} onChange={e => { setJobClass(e.target.value); setRouteKey('') }} placeholder="Choose a class" options={view.management.job_classes.map(c => ({ value: c, label: c }))} />
    <Select label="Qualified route" value={routeKey} onChange={e => setRouteKey(e.target.value)} placeholder="Choose a route" options={view.routes.filter(r => r.route.action_type === jobClass).map(r => ({ value: JSON.stringify(r.route), label: `${r.route.provider} / ${r.route.action_type}` }))} />
    <Input label="Maximum calls per root" type="number" min="1" value={units} onChange={e => setUnits(e.target.value)} />
    <Input label="Maximum concurrent calls" type="number" min="1" value={concurrent} onChange={e => setConcurrent(e.target.value)} />
    <Input label="Mandate deadline" type="datetime-local" value={deadline} onChange={e => setDeadline(e.target.value)} />
    </div><fieldset><legend>Eligible authenticated requesters</legend><div className={styles.actions}>{view.management.principals.map(p => <label key={p.id}><input type="checkbox" checked={initiators.includes(p.id)} onChange={() => setInitiators(toggle(initiators, p.id))} /> {p.id}</label>)}</div></fieldset>
    <fieldset><legend>Required relationship dependencies</legend><p className={styles.muted}>A personal agent representing a team requires its owner’s requester membership and its own duty assignment.</p><div className={styles.actions}>{choices.map(c => <label key={c.key}><input type="checkbox" checked={dependencies.includes(c.key)} onChange={() => setDependencies(toggle(dependencies, c.key))} /> {c.label}</label>)}</div></fieldset>
    {error && <p className={styles.error} role="alert">{error}</p>}<Button disabled={disabled} onClick={() => submit(Date.now())}>Review mandate</Button></section>
}
