import { memo } from 'react'
import { Handle, Position, type NodeProps } from '@xyflow/react'
import { cn } from '../../lib/cn'
import { Layers, Loader2 } from 'lucide-react'
import styles from './ParallelNode.module.css'

interface ParallelSubStep {
  name: string
  status: string
  error?: string
}

interface ParallelNodeData {
  label: string
  status: string
  isActive: boolean
  isExecuted: boolean
  joinPolicy: string
  subSteps: ParallelSubStep[]
  [key: string]: unknown
}

function statusDotClass(status: string): string {
  switch (status) {
    case 'completed': return styles.dotCompleted
    case 'failed': return styles.dotFailed
    case 'running':
    case 'waiting_parallel': return styles.dotRunning
    default: return styles.dotPending
  }
}

export const ParallelNode = memo(function ParallelNode({ data }: NodeProps) {
  const d = data as ParallelNodeData

  const statusClass =
    d.status === 'completed' ? styles.nodeCompleted :
    d.status === 'failed' ? styles.nodeFailed :
    d.status === 'running' || d.status === 'waiting_parallel' ? styles.nodeRunning :
    styles.nodePending

  return (
    <>
      <Handle type="target" position={Position.Top} className={styles.handle} />
      <div
        className={cn(
          styles.node,
          statusClass,
          d.isActive && styles.nodeActive,
        )}
      >
        <div className={styles.header}>
          {d.isActive
            ? <Loader2 className="h-4 w-4 text-primary-400 animate-spin shrink-0" />
            : <Layers className={styles.icon} />}
          <span className={styles.label}>{d.label}</span>
          <span className={styles.joinBadge}>{d.joinPolicy}</span>
        </div>

        {d.subSteps.length > 0 && (
          <ul className={styles.subStepList} aria-label="Parallel sub-steps">
            {d.subSteps.map((sub) => (
              <li key={sub.name} className={styles.subStepRow}>
                <span
                  className={cn(styles.statusDot, statusDotClass(sub.status))}
                  aria-label={`Status: ${sub.status}`}
                />
                <span className={styles.subStepName}>{sub.name}</span>
                {sub.error && (
                  <span className={styles.subStepError} title={sub.error}>{sub.error}</span>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
      <Handle type="source" position={Position.Bottom} className={styles.handle} />
    </>
  )
})
