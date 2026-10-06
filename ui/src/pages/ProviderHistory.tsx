import { useState } from 'react'
import { useProviderExecutionHistory } from '../api/hooks/useGovernance'
import { Input } from '../components/ui/Input'
import { Button } from '../components/ui/Button'
import { Badge } from '../components/ui/Badge'
import { JsonViewer } from '../components/ui/JsonViewer'
import styles from './Governance.module.css'

const statusLabels = {
  prepared: 'Prepared',
  in_flight: 'In flight',
  awaiting_retry: 'Awaiting retry',
  completed: 'Completed',
  reconciliation_required: 'Reconciliation required',
}

export function ProviderHistory({ namespace, tenant }: { namespace: string; tenant: string }) {
  const [input, setInput] = useState('')
  const [executionId, setExecutionId] = useState('')
  const query = useProviderExecutionHistory(namespace, tenant, executionId)
  const history = query.isError ? undefined : query.data
  return <section className={`${styles.card} ${styles.history}`}>
    <h2>Provider execution history</h2>
    <p className={styles.muted}>Inspect retained receipts and how each attempt was resolved.</p>
    <form className={styles.historySearch} onSubmit={event => {
      event.preventDefault()
      const next = input.trim().toLowerCase()
      if (next === executionId) void query.refetch()
      else setExecutionId(next)
    }}>
      <Input label="Execution ID" placeholder="Provider execution UUID" value={input}
        onChange={event => setInput(event.target.value)} required maxLength={36}
        pattern="[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}" />
      <Button loading={query.isFetching}>Read receipt</Button>
    </form>
    {query.error && <p className={styles.error} role="alert">{query.error.message}</p>}
    {history && <>
      <div className={styles.row}>
        <div className={styles.identity}>
          <strong>{history.subject.id} · {history.subject.kind}</strong>
          <p className={styles.muted}>{history.receipt.execution_id}</p>
          <p className={styles.muted}>{history.receipt.attempts} registered attempts · {history.operation_integrity} operation</p>
        </div>
        <Badge variant={history.receipt.status.state === 'completed' ? 'info' : 'warning'}>
          {statusLabels[history.receipt.status.state]}
        </Badge>
      </div>
      <p className={styles.muted}>Observed authority generation {history.observed_authority.generation}. This receipt does not grant execution permission.</p>
      {history.cancellation_fenced && <Badge variant="warning">New execution blocked by cancellation</Badge>}
      {history.metadata && <p className={styles.muted}>Original action {history.metadata.original_action_id} · up to {history.metadata.max_attempts} attempts</p>}
      {history.binding && <p className={styles.muted}>Provider {history.binding.provider} · revision {history.binding.provider_revision}</p>}
      {history.receipt.status.state === 'completed' && <details>
        <summary>Recorded outcome</summary>
        <JsonViewer data={history.receipt.status.outcome} collapsed />
      </details>}
      {history.attempts.map(attempt => <article className={styles.card} key={attempt.attempt_id}>
        <div className={styles.row}><strong>Attempt {attempt.ordinal + 1}</strong><Badge variant="neutral">{attempt.ledger_status}</Badge></div>
        <p className={styles.muted}>{attempt.attempt_id}</p>
        {attempt.original_outcome !== null ? <details>
          <summary>Original verified result</summary>
          <JsonViewer data={{ evidence: attempt.original_evidence, outcome: attempt.original_outcome }} collapsed />
        </details> : <p className={styles.muted}>No verified original result.</p>}
        {attempt.reconciliation && <details>
          <summary>{attempt.reconciliation.acceptance
            ? `Accepted by ${attempt.reconciliation.acceptance.operator.id} · ${new Date(attempt.reconciliation.acceptance.accepted_at_ms).toLocaleString()}`
            : 'Verified adapter finality'}</summary>
          <p className={styles.muted}>Proof recorded {new Date(attempt.reconciliation.resolved_at_ms).toLocaleString()}.</p>
          {attempt.reconciliation.acceptance
            ? <p className={styles.muted}>Operator {attempt.reconciliation.acceptance.operator.kind} · authority generation {attempt.reconciliation.acceptance.authority.generation}.</p>
            : <p className={styles.muted}>No operator acceptance record was retained for this settlement.</p>}
          <JsonViewer data={attempt.reconciliation} collapsed />
        </details>}
      </article>)}
      {history.attempts.length === 0 && <p className={styles.muted}>No provider attempt was registered.</p>}
    </>}
  </section>
}
