import { useCallback, useState } from 'react'
import { useParams, useNavigate, useSearchParams, Link } from 'react-router-dom'
import { useQueryClient } from '@tanstack/react-query'
import { XCircle, ArrowUpRight, GitBranch, Layers, Radio, History } from 'lucide-react'
import {
  useChainDetail,
  useCancelChain,
  useChainDag,
  useExecutionHistory,
  useSignalExecution,
} from '../api/hooks/useChains'
import { useEntityStream } from '../api/hooks/useEntityStream'
import { PageHeader } from '../components/layout/PageHeader'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Modal } from '../components/ui/Modal'
import { JsonViewer } from '../components/ui/JsonViewer'
import { Skeleton } from '../components/ui/Skeleton'
import { ChainDAG } from '../components/dag/ChainDAG'
import { useToast } from '../components/ui/useToast'
import { absoluteTime, formatCountdown } from '../lib/format'
import styles from './ChainDetail.module.css'

export function ChainDetail() {
  const { chainId } = useParams<{ chainId: string }>()
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()
  const ns = searchParams.get('namespace') ?? ''
  const tenant = searchParams.get('tenant') ?? ''
  const expandAll = searchParams.get('expand') === 'all'
  const { data: chain, isLoading } = useChainDetail(chainId, { namespace: ns, tenant })
  const cancel = useCancelChain()
  const { toast } = useToast()
  const qc = useQueryClient()
  const [cancelOpen, setCancelOpen] = useState(false)
  const [selectedStep, setSelectedStep] = useState<string | null>(null)
  const [signalOpen, setSignalOpen] = useState(false)
  const [signalName, setSignalName] = useState('')
  const [signalPayload, setSignalPayload] = useState('')

  const { data: dag } = useChainDag(chainId, { namespace: ns, tenant })
  const { data: history } = useExecutionHistory(chainId, { namespace: ns, tenant })
  const signal = useSignalExecution()

  const handleStreamEvent = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ['chain', chainId] })
  }, [qc, chainId])

  useEntityStream({
    entityType: 'chain',
    entityId: chainId,
    namespace: ns,
    tenant,
    enabled: !!chainId && !!ns && !!tenant,
    onEvent: handleStreamEvent,
  })

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
      { chainId: chain.chain_id, namespace: ns, tenant },
      {
        onSuccess: () => { toast('success', 'Chain cancelled'); setCancelOpen(false) },
        onError: (e) => toast('error', 'Cancel failed', (e as Error).message),
      },
    )
  }

  const handleSignal = () => {
    let payload: unknown
    if (signalPayload.trim()) {
      try {
        payload = JSON.parse(signalPayload)
      } catch {
        toast('error', 'Invalid JSON payload')
        return
      }
    }
    signal.mutate(
      { executionId: chain.chain_id, signalName: signalName.trim(), namespace: ns, tenant, payload },
      {
        onSuccess: () => { toast('success', `Signal "${signalName.trim()}" delivered`); setSignalOpen(false) },
        onError: (e) => toast('error', 'Signal failed', (e as Error).message),
      },
    )
  }

  const isActive = ['running', 'waiting_sub_chain', 'waiting_parallel', 'waiting_timer', 'waiting_signal', 'waiting_worker'].includes(chain.status)

  return (
    <div>
      <PageHeader
        title={`Chain: ${chain.chain_name}`}
        subtitle={`${chain.chain_id} -- Started ${absoluteTime(chain.started_at)}`}
        actions={
          <div className={styles.headerActions}>
            <Badge size="md">{chain.status}</Badge>
            {isActive && (
              <Button
                variant="secondary"
                size="sm"
                icon={<Radio className="h-3.5 w-3.5" />}
                onClick={() => setSignalOpen(true)}
              >
                Send Signal
              </Button>
            )}
            {isActive && (
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

      {chain.parent_chain_id && (
        <div className={styles.parentChainLink}>
          <ArrowUpRight className="h-4 w-4 text-gray-500" />
          <span className="text-sm text-gray-500">Parent Chain:</span>
          <Link
            to={`/chains/${chain.parent_chain_id}`}
            className="text-sm text-primary-400 hover:underline"
          >
            {chain.parent_chain_id.slice(0, 12)}...
          </Link>
        </div>
      )}

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

      <ChainDAG
        chain={chain}
        dag={dag}
        onSelectStep={setSelectedStep}
        onNavigateChain={(id) => navigate(`/chains/${id}`)}
        defaultExpandAll={expandAll}
      />

      {step && (
        <div className={styles.stepDetailCard}>
          <div className={styles.stepHeader}>
            <h3 className={styles.stepTitle}>Step: {step.name}</h3>
            <Badge>{step.status}</Badge>
          </div>
          <div className={styles.stepMetadata}>
            <div><span className="text-gray-500">Provider:</span> {step.provider}</div>
            {step.completed_at && <div><span className="text-gray-500">Completed:</span> {absoluteTime(step.completed_at)}</div>}
            {step.attempt !== undefined && step.max_retries !== undefined && step.attempt > 1 && (
              <div>
                <span className="text-gray-500">Attempt:</span>{' '}
                <span className={`inline-flex items-center gap-1 text-xs font-medium px-1.5 py-0.5 rounded ${step.status === 'failed' ? 'bg-red-500/15 text-red-400' : 'bg-amber-500/15 text-amber-400'}`}>
                  &#x21BB; {step.attempt} / {step.max_retries + 1}
                </span>
              </div>
            )}
            {step.error && <div className={styles.stepError}><span className="text-gray-500">Error:</span> {step.error}</div>}
          </div>
          {step.sub_chain && (
            <div className={styles.subChainInfo}>
              <GitBranch className="h-4 w-4 text-primary-400" />
              <span className="text-sm text-gray-500">Sub-chain:</span>
              <span className="text-sm font-medium">{step.sub_chain}</span>
              {step.child_chain_id && (
                <Link
                  to={`/chains/${step.child_chain_id}`}
                  className="text-sm text-primary-400 hover:underline ml-2"
                >
                  View child chain
                </Link>
              )}
            </div>
          )}
          {step.parallel_sub_steps && step.parallel_sub_steps.length > 0 && (
            <div className={styles.parallelStepsSection}>
              <div className={styles.parallelStepsHeader}>
                <Layers className="h-4 w-4 text-primary-400" />
                <span className="text-sm text-gray-500">Parallel sub-steps</span>
              </div>
              <ul className={styles.parallelStepsList}>
                {step.parallel_sub_steps.map((sub) => (
                  <li key={sub.name} className={styles.parallelStepItem}>
                    <div className={styles.parallelStepItemHeader}>
                      <span className="text-sm font-medium">{sub.name}</span>
                      <Badge size="sm">{sub.status}</Badge>
                    </div>
                    {sub.error && (
                      <p className={styles.parallelStepError}>{sub.error}</p>
                    )}
                    {sub.response_body && (
                      <div className={styles.responseSection}>
                        <JsonViewer data={sub.response_body} collapsed />
                      </div>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}
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

      {history && history.events.length > 0 && (
        <div className={styles.stepDetailCard}>
          <div className={styles.stepHeader}>
            <h3 className={styles.stepTitle}>
              <History className="inline h-4 w-4 mr-1 text-primary-400" />
              Event History
            </h3>
            <span className="text-xs text-gray-500">{history.events.length} events</span>
          </div>
          <ul className="space-y-1">
            {history.events.map((event) => {
              const detail = [
                typeof event.step_name === 'string' && event.step_name,
                typeof event.signal_name === 'string' && `signal: ${event.signal_name}`,
                typeof event.queue === 'string' && `queue: ${event.queue}`,
                typeof event.fire_at === 'string' && `fires ${absoluteTime(event.fire_at)}`,
                typeof event.error === 'string' && event.error && `error: ${event.error}`,
              ]
                .filter(Boolean)
                .join(' -- ')
              return (
                <li key={event.event_id} className="flex items-baseline gap-2 text-sm">
                  <span className="text-xs text-gray-600 w-8 text-right shrink-0">#{event.event_id}</span>
                  <span className="text-xs text-gray-500 w-40 shrink-0">{absoluteTime(event.timestamp)}</span>
                  <Badge variant={event.event_type.includes('failed') || event.event_type.includes('timed_out') ? 'error' : 'info'} size="sm">
                    {event.event_type}
                  </Badge>
                  {detail && <span className="text-gray-400 truncate">{detail}</span>}
                </li>
              )
            })}
          </ul>
        </div>
      )}

      <Modal
        open={signalOpen}
        onClose={() => setSignalOpen(false)}
        title="Send Signal"
        footer={
          <>
            <Button variant="secondary" onClick={() => setSignalOpen(false)}>Cancel</Button>
            <Button variant="primary" loading={signal.isPending} disabled={!signalName.trim()} onClick={handleSignal}>
              Deliver
            </Button>
          </>
        }
      >
        <div className="space-y-3">
          <p className="text-sm text-gray-400">
            Deliver an external signal to this execution. If it is paused on a matching
            wait step it resumes immediately; otherwise the signal is buffered.
          </p>
          <input
            className="w-full rounded border border-gray-700 bg-transparent px-2 py-1.5 text-sm"
            placeholder="Signal name (e.g. approved)"
            value={signalName}
            onChange={(e) => setSignalName(e.target.value)}
          />
          <textarea
            className="w-full rounded border border-gray-700 bg-transparent px-2 py-1.5 text-sm font-mono"
            rows={4}
            placeholder='Optional JSON payload, e.g. {"approver": "renzo"}'
            value={signalPayload}
            onChange={(e) => setSignalPayload(e.target.value)}
          />
        </div>
      </Modal>

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
