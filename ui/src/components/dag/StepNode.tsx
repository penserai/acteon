import { memo } from 'react'
import { Handle, Position, type NodeProps } from '@xyflow/react'
import { cn } from '../../lib/cn'
import { CheckCircle2, XCircle, Loader2, Circle, SkipForward } from 'lucide-react'
import styles from './StepNode.module.css'

interface StepNodeData {
  label: string
  status: string
  isActive: boolean
  isExecuted: boolean
  error?: string
  [key: string]: unknown
}

const statusIcons: Record<string, React.ReactNode> = {
  completed: <CheckCircle2 className="h-4 w-4 text-success-500" />,
  failed: <XCircle className="h-4 w-4 text-error-500" />,
  pending: <Circle className="h-4 w-4 text-gray-400" />,
  skipped: <SkipForward className="h-4 w-4 text-gray-400" />,
}

export const StepNode = memo(function StepNode({ data }: NodeProps) {
  const d = data as StepNodeData
  const statusClass = d.status === 'completed' ? styles.nodeCompleted :
                      d.status === 'failed' ? styles.nodeFailed :
                      d.status === 'skipped' ? styles.nodeSkipped :
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
        <div className={styles.content}>
          {d.isActive ? <Loader2 className="h-4 w-4 text-primary-400 animate-spin" /> : statusIcons[d.status]}
          <span className={styles.label}>{d.label}</span>
        </div>
        {d.error && (
          <p className={styles.error}>{d.error}</p>
        )}
      </div>
      <Handle type="source" position={Position.Bottom} className={styles.handle} />
    </>
  )
})
