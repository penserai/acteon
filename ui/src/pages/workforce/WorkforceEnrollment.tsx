import { useState } from 'react'
import { Input } from '../../components/ui/Input'
import { Select } from '../../components/ui/Select'
import { Button } from '../../components/ui/Button'
import type { RepresentedParty, TeamRole, WorkforceChange, WorkforceScopeView } from '../../types'
import type { WorkforceReview } from '../Workforce'
import styles from '../Governance.module.css'

type Enrollment = 'team' | 'membership' | 'ownership' | 'assignment'
export function WorkforceEnrollment({ view, review, disabled }: { view: WorkforceScopeView; review: WorkforceReview; disabled: boolean }) {
  const [kind, setKind] = useState<Enrollment>('team')
  const [teamKey, setTeamKey] = useState('')
  const [id, setId] = useState('')
  const [name, setName] = useState('')
  const [humanId, setHumanId] = useState('')
  const [agentId, setAgentId] = useState('')
  const [ownerKind, setOwnerKind] = useState<'human' | 'team'>('team')
  const [role, setRole] = useState<TeamRole>('requester')
  const [jobClass, setJobClass] = useState('')
  const [deadline, setDeadline] = useState('')
  const [error, setError] = useState('')
  const team = view.management.teams.find(t => JSON.stringify(t) === teamKey)
  const human = view.management.principals.find(p => p.id === humanId && p.kind === 'human')
  const agent = view.management.principals.find(p => p.id === agentId && p.kind === 'agent')
  const submit = (now: number) => {
    let change: WorkforceChange
    const deadlineMs = Date.parse(deadline)
    if ((kind === 'membership' || kind === 'assignment') && (!Number.isSafeInteger(deadlineMs) || deadlineMs <= now || deadlineMs > view.management.limits.deadline_ms)) {
      setError('Choose a future deadline within your management bounds.'); return
    }
    if (kind !== 'ownership' || ownerKind === 'team') {
      if (!team) { setError('Choose an authorized team.'); return }
    }
    switch (kind) {
      case 'team': {
        if (!team || !name.trim()) { setError('Choose a team and enter its name.'); return }
        const existing = view.teams.find(r => JSON.stringify(r.value.team) === teamKey)
        if (existing?.revoked) { setError('A disbanded team identity cannot be reused.'); return }
        change = { kind: 'put_team', team: { team, name: name.trim(), revision: (existing?.value.revision ?? 0) + 1 } }; break
      }
      case 'membership': {
        if (!team || !human || !id.trim()) { setError('Choose a team, a human and a membership ID.'); return }
        const existing = view.memberships.find(r => r.value.id === id.trim())
        if (existing?.revoked) { setError('Use a new ID for a removed membership.'); return }
        change = { kind: 'put_membership', membership: { id: id.trim(), revision: (existing?.value.revision ?? 0) + 1,
          team, human, roles: [role], valid_from_ms: view.management.valid_from_ms, deadline_ms: deadlineMs } }; break
      }
      case 'ownership': {
        if (!agent || (ownerKind === 'human' ? !human : !team)) { setError('Choose an agent and its owner.'); return }
        const owner: RepresentedParty = ownerKind === 'human' ? { kind: 'human', principal: human! } : { kind: 'team', team: team! }
        const existing = view.ownership.find(r => r.value.agent.id === agent.id)
        change = { kind: 'put_ownership', ownership: { agent, owner, revision: (existing?.value.revision ?? 0) + 1 } }; break
      }
      case 'assignment': {
        if (!team || !agent || !id.trim() || !view.management.job_classes.includes(jobClass)) { setError('Choose a team, agent and duty, and enter an assignment ID.'); return }
        const existing = view.assignments.find(r => r.value.id === id.trim())
        if (existing?.revoked) { setError('Use a new ID for a removed assignment.'); return }
        change = { kind: 'put_assignment', assignment: { id: id.trim(), revision: (existing?.value.revision ?? 0) + 1,
          team, agent, job_classes: [jobClass], valid_from_ms: view.management.valid_from_ms, deadline_ms: deadlineMs } }; break
      }
    }
    setError(''); review(`Save ${kind} ${kind === 'ownership' ? agent?.id : kind === 'team' ? name.trim() : id.trim()}`, change)
  }
  const teams = view.management.teams.filter(t => kind === 'team' || view.teams.some(r => !r.revoked && JSON.stringify(r.value.team) === JSON.stringify(t)))
  return <section className={styles.card}><h2>Enroll and update the workforce</h2><p className={styles.muted}>Membership, stewardship and availability are recorded separately from execution authority. Existing records advance by one revision.</p><div className={styles.grid}>
    <Select label="Record" value={kind} onChange={e => { setKind(e.target.value as Enrollment); setError('') }} options={[
      { value: 'team', label: 'Team' }, { value: 'membership', label: 'Human membership' }, { value: 'ownership', label: 'Agent ownership' }, { value: 'assignment', label: 'Duty assignment' }]} />
    {(kind !== 'ownership' || ownerKind === 'team') && <Select label="Team" value={teamKey} onChange={e => setTeamKey(e.target.value)} placeholder="Choose a team" options={teams.map(t => ({ value: JSON.stringify(t), label: `${t.domain} / ${t.id}` }))} />}
    {kind === 'team' && <Input label="Team name" value={name} onChange={e => setName(e.target.value)} />}
    {(kind === 'membership' || kind === 'assignment') && <Input label={kind === 'membership' ? 'Membership ID' : 'Assignment ID'} value={id} onChange={e => setId(e.target.value)} />}
    {kind === 'ownership' && <Select label="Owner type" value={ownerKind} onChange={e => setOwnerKind(e.target.value as 'human' | 'team')} options={[{ value: 'team', label: 'Team' }, { value: 'human', label: 'Human' }]} />}
    {(kind === 'membership' || (kind === 'ownership' && ownerKind === 'human')) && <Select label="Human" value={humanId} onChange={e => setHumanId(e.target.value)} placeholder="Choose a human" options={view.management.principals.filter(p => p.kind === 'human').map(p => ({ value: p.id, label: p.id }))} />}
    {(kind === 'ownership' || kind === 'assignment') && <Select label="Agent" value={agentId} onChange={e => setAgentId(e.target.value)} placeholder="Choose an agent" options={view.management.principals.filter(p => p.kind === 'agent').map(p => ({ value: p.id, label: p.id }))} />}
    {kind === 'membership' && <Select label="Team role" value={role} onChange={e => setRole(e.target.value as TeamRole)} options={[
      { value: 'requester', label: 'Requester' }, { value: 'approver', label: 'Approver' }, { value: 'workforce_manager', label: 'Workforce manager' }, { value: 'mandate_issuer', label: 'Mandate issuer' }]} />}
    {kind === 'assignment' && <Select label="Duty" value={jobClass} onChange={e => setJobClass(e.target.value)} placeholder="Choose a job class" options={view.management.job_classes.map(c => ({ value: c, label: c }))} />}
    {(kind === 'membership' || kind === 'assignment') && <Input label="Relationship deadline" type="datetime-local" value={deadline} onChange={e => setDeadline(e.target.value)} />}
    </div>{error && <p className={styles.error} role="alert">{error}</p>}<Button disabled={disabled} onClick={() => submit(Date.now())}>Review workforce record</Button></section>
}
