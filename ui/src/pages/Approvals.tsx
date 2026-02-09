import { useState, useEffect } from 'react'
import { useApprovals, useApproveAction, useRejectAction } from '../api/hooks/useApprovals'
import { PageHeader } from '../components/layout/PageHeader'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { JsonViewer } from '../components/ui/JsonViewer'
import { EmptyState } from '../components/ui/EmptyState'
import { Skeleton } from '../components/ui/Skeleton'
import { useToast } from '../components/ui/useToast'
import { relativeTime, formatCountdown } from '../lib/format'
import type { ApprovalStatus } from '../types'
import { ShieldCheck, ChevronDown, ChevronUp } from 'lucide-react'
import styles from './Approvals.module.css'

export function Approvals() {
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')
  const { data: approvals, isLoading } = useApprovals({ namespace: ns || undefined, tenant: tenant || undefined })
  const approve = useApproveAction()
  const reject = useRejectAction()
  const { toast } = useToast()

  const handleApprove = (a: ApprovalStatus) => {
    approve.mutate(
      { ns: a.namespace ?? '', tenant: a.tenant ?? '', id: a.approval_id },
      {
        onSuccess: () => toast('success', 'Action approved'),
        onError: (e) => toast('error', 'Approve failed', (e as Error).message),
      },
    )
  }

  const handleReject = (a: ApprovalStatus) => {
    reject.mutate(
      { ns: a.namespace ?? '', tenant: a.tenant ?? '', id: a.approval_id },
      {
        onSuccess: () => toast('success', 'Action rejected'),
        onError: (e) => toast('error', 'Reject failed', (e as Error).message),
      },
    )
  }

  const pending = (approvals ?? []).filter((a) => a.status === 'pending')

  return (
    <div>
      <PageHeader
        title="Approvals"
        subtitle={pending.length > 0 ? `${pending.length} pending` : undefined}
      />

      <div className={styles.filterContainer}>
        <Input placeholder="Namespace" value={ns} onChange={(e) => setNs(e.target.value)} />
        <Input placeholder="Tenant" value={tenant} onChange={(e) => setTenant(e.target.value)} />
      </div>

      {isLoading ? (
        <div className={styles.loadingContainer}>
          {Array.from({ length: 3 }).map((_, i) => <Skeleton key={i} className="h-44" />)}
        </div>
      ) : pending.length === 0 ? (
        <EmptyState
          icon={<ShieldCheck className="h-12 w-12" />}
          title="No pending approvals"
          description="Actions requiring approval will appear here when a RequestApproval rule matches."
        />
      ) : (
        <div className={styles.approvalsList}>
          {pending.map((a) => (
            <ApprovalCard key={a.approval_id} approval={a} onApprove={handleApprove} onReject={handleReject} />
          ))}
        </div>
      )}
    </div>
  )
}

function ApprovalCard({ approval, onApprove, onReject }: {
  approval: ApprovalStatus
  onApprove: (a: ApprovalStatus) => void
  onReject: (a: ApprovalStatus) => void
}) {
  const [showPayload, setShowPayload] = useState(false)
  const [countdown, setCountdown] = useState(formatCountdown(approval.expires_at))

  useEffect(() => {
    const timer = setInterval(() => setCountdown(formatCountdown(approval.expires_at)), 1000)
    return () => clearInterval(timer)
  }, [approval.expires_at])

  return (
    <article
      aria-label={`Approval for ${approval.action_id}`}
      className={styles.approvalCard}
    >
      <div className={styles.cardHeader}>
        <Badge variant="warning" size="md">PendingApproval</Badge>
        <span className={styles.timestamp}>{relativeTime(approval.created_at)}</span>
      </div>

      <div className={styles.detailsContainer}>
        <p><span className={styles.detailLabel}>Action:</span> {approval.action_id.slice(0, 16)}...</p>
        <p><span className={styles.detailLabel}>Rule:</span> {approval.rule}</p>
        {approval.message && <p><span className={styles.detailLabel}>Message:</span> {approval.message}</p>}
      </div>

      <div className={styles.metadataRow}>
        {approval.namespace && <span>ns: {approval.namespace}</span>}
        {approval.tenant && <span>tenant: {approval.tenant}</span>}
        <span>Expires: {countdown}</span>
      </div>

      {approval.payload && (
        <button
          onClick={() => setShowPayload(!showPayload)}
          className={styles.toggleButton}
        >
          {showPayload ? <ChevronUp className="h-3 w-3" /> : <ChevronDown className="h-3 w-3" />}
          {showPayload ? 'Hide' : 'Show'} payload
        </button>
      )}

      {showPayload && approval.payload && (
        <div className={styles.payloadContainer}>
          <JsonViewer data={approval.payload} collapsed />
        </div>
      )}

      <div className={styles.actionButtons}>
        <Button variant="danger" size="md" onClick={() => onReject(approval)}>Reject</Button>
        <Button variant="success" size="md" onClick={() => onApprove(approval)}>Approve</Button>
      </div>
    </article>
  )
}
