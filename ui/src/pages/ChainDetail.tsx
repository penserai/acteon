import { useState } from 'react'
import { useParams } from 'react-router-dom'
import { XCircle } from 'lucide-react'
import { useChainDetail, useCancelChain } from '../api/hooks/useChains'
import { PageHeader } from '../components/layout/PageHeader'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Modal } from '../components/ui/Modal'
import { JsonViewer } from '../components/ui/JsonViewer'
import { Skeleton } from '../components/ui/Skeleton'
import { ChainDAG } from '../components/dag/ChainDAG'
import { useToast } from '../components/ui/Toast'
import { absoluteTime, formatCountdown } from '../lib/format'
import styles from './ChainDetail.module.css'

export function ChainDetail() {
  const { chainId } = useParams<{ chainId: string }>()
  const { data: chain, isLoading } = useChainDetail(chainId)
  const cancel = useCancelChain()
  const { toast } = useToast()
  const [cancelOpen, setCancelOpen] = useState(false)
  const [selectedStep, setSelectedStep] = useState<string | null>(null)

  if (isLoading || !chain) {
    return (
      <div className={styles.skeletonContainer}>
        <Skeleton className={styles.skeletonTitle} />
        <Skeleton className={styles.skeletonContent} />
      </div>
    )
  }

  const step = chain.steps.find((s) => s.name === selectedStep)

  const handleCancel = () => {
    cancel.mutate(
      { chainId: chain.chain_id, namespace: '', tenant: '' },
      {
        onSuccess: () => { toast('success', 'Chain cancelled'); setCancelOpen(false) },
        onError: (e) => toast('error', 'Cancel failed', (e as Error).message),
      },
    )
  }

  return (
    <div>
      <PageHeader
        title={`Chain: ${chain.chain_name}`}
        subtitle={`${chain.chain_id} -- Started ${absoluteTime(chain.started_at)}`}
        actions={
          <div className={styles.headerActions}>
            <Badge size="md">{chain.status}</Badge>
            {chain.status === 'running' && (
              <Button
                variant="danger"
                size="sm"
                icon={<XCircle className="h-3.5 w-3.5" />}
                onClick={() => setCancelOpen(true)}
              >
                Cancel
              </Button>
            )}
          </div>
        }
      />

      {chain.execution_path.length > 0 && (
        <div className={styles.executionPath}>
          <span className={styles.executionPathLabel}>Execution Path:</span>
          {chain.execution_path.map((name, i) => (
            <span key={name} className={styles.executionPathSteps}>
              {i > 0 && <span className={styles.executionPathArrow}>-&gt;</span>}
              <Badge variant="info" size="sm">{name}</Badge>
            </span>
          ))}
        </div>
      )}

      {chain.expires_at && chain.status === 'running' && (
        <p className={styles.expiresMessage}>Expires: {formatCountdown(chain.expires_at)}</p>
      )}

      <ChainDAG chain={chain} onSelectStep={setSelectedStep} />

      {step && (
        <div className={styles.stepDetailCard}>
          <div className={styles.stepHeader}>
            <h3 className={styles.stepTitle}>Step: {step.name}</h3>
            <Badge>{step.status}</Badge>
          </div>
          <div className={styles.stepMetadata}>
            <div><span className="text-gray-500">Provider:</span> {step.provider}</div>
            {step.completed_at && <div><span className="text-gray-500">Completed:</span> {absoluteTime(step.completed_at)}</div>}
            {step.error && <div className={styles.stepError}><span className="text-gray-500">Error:</span> {step.error}</div>}
          </div>
          {step.response_body && (
            <div>
              <span className={styles.responseLabel}>Response:</span>
              <div className={styles.responseSection}>
                <JsonViewer data={step.response_body} collapsed />
              </div>
            </div>
          )}
        </div>
      )}

      <Modal
        open={cancelOpen}
        onClose={() => setCancelOpen(false)}
        title="Cancel Chain"
        footer={
          <>
            <Button variant="secondary" onClick={() => setCancelOpen(false)}>Cancel</Button>
            <Button variant="danger" loading={cancel.isPending} onClick={handleCancel}>Confirm Cancel</Button>
          </>
        }
      >
        <p>Cancel chain <strong>{chain.chain_name}</strong> ({chain.chain_id.slice(0, 12)})?</p>
        <p className={styles.modalContent}>This will stop execution at the current step.</p>
      </Modal>
    </div>
  )
}
