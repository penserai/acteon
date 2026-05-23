import { useState, useEffect } from 'react'
import { useApprovals, useApproveAction, useRejectAction } from '../api/hooks/useApprovals'
import { PageHeader } from '../components/layout/PageHeader'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { EmptyState } from '../components/ui/EmptyState'
import { Skeleton } from '../components/ui/Skeleton'
import { useToast } from '../components/ui/useToast'
import { relativeTime, formatCountdown } from '../lib/format'
import type { ApprovalStatus } from '../types'
import { ShieldCheck } from 'lucide-react'
import shared from '../styles/shared.module.css'
import styles from './Approvals.module.css'

export function Approvals() {
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')
  const { data: approvals, isLoading } = useApprovals({ namespace: ns || undefined, tenant: tenant || undefined })
  const approve = useApproveAction()
  const reject = useRejectAction()
  const { toast } = useToast()

  const handleApprove = (a: ApprovalStatus) => {
    if (!ns || !tenant) {
      toast('error', 'Namespace and tenant are required to approve')
      return
    }
    approve.mutate(
      { ns, tenant, id: a.token },
      {
        onSuccess: () => toast('success', 'Action approved'),
        onError: (e) => toast('error', 'Approve failed', (e as Error).message),
      },
    )
  }

  const handleReject = (a: ApprovalStatus) => {
    if (!ns || !tenant) {
      toast('error', 'Namespace and tenant are required to reject')
      return
    }
    reject.mutate(
      { ns, tenant, id: a.token },
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
            <ApprovalCard key={a.token} approval={a} onApprove={handleApprove} onReject={handleReject} />
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
  const [countdown, setCountdown] = useState(formatCountdown(approval.expires_at))

  useEffect(() => {
    const timer = setInterval(() => setCountdown(formatCountdown(approval.expires_at)), 1000)
    return () => clearInterval(timer)
  }, [approval.expires_at])

  return (
    <article
      aria-label={`Approval ${approval.token.slice(0, 8)}`}
      className={styles.approvalCard}
    >
      <div className={styles.cardHeader}>
        <Badge variant="warning" size="md">PendingApproval</Badge>
        <span className={styles.timestamp}>{relativeTime(approval.created_at)}</span>
      </div>

      <div className={styles.detailsContainer}>
        <p><span className={shared.detailLabel}>Token:</span> {approval.token.slice(0, 16)}...</p>
        <p><span className={shared.detailLabel}>Rule:</span> {approval.rule}</p>
        {approval.message && <p><span className={shared.detailLabel}>Message:</span> {approval.message}</p>}
      </div>

      <div className={styles.metadataRow}>
        <span>Expires: {countdown}</span>
      </div>

      <div className={styles.actionButtons}>
        <Button variant="danger" size="md" onClick={() => onReject(approval)}>Reject</Button>
        <Button variant="success" size="md" onClick={() => onApprove(approval)}>Approve</Button>
      </div>
    </article>
  )
}
