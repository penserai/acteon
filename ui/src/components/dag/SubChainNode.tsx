import { memo } from 'react'
import { Handle, Position, type NodeProps } from '@xyflow/react'
import { cn } from '../../lib/cn'
import { GitBranch, ChevronDown, ChevronRight, Loader2, CheckCircle2, XCircle, Circle } from 'lucide-react'
import styles from './SubChainNode.module.css'

interface SubChainNodeData {
  label: string
  status: string
  isActive: boolean
  isExecuted: boolean
  subChainName: string
  childChainId?: string
  childStepCount?: number
  expanded: boolean
  onToggleExpand?: (name: string) => void
  onNavigateChild?: (chainId: string) => void
  [key: string]: unknown
}

const statusIcons: Record<string, React.ReactNode> = {
  completed: <CheckCircle2 className="h-3.5 w-3.5 text-success-500" />,
  failed: <XCircle className="h-3.5 w-3.5 text-error-500" />,
  running: <Loader2 className="h-3.5 w-3.5 text-primary-400 animate-spin" />,
  waiting_sub_chain: <Loader2 className="h-3.5 w-3.5 text-primary-400 animate-spin" />,
  pending: <Circle className="h-3.5 w-3.5 text-gray-400" />,
}

export const SubChainNode = memo(function SubChainNode({ data }: NodeProps) {
  const d = data as SubChainNodeData
  const statusClass =
    d.status === 'completed' ? styles.nodeCompleted :
    d.status === 'failed' ? styles.nodeFailed :
    d.status === 'running' || d.status === 'waiting_sub_chain' ? styles.nodeRunning :
    styles.nodePending

  const handleToggle = (e: React.MouseEvent) => {
    e.stopPropagation()
    d.onToggleExpand?.(d.label)
  }

  const handleNavigate = (e: React.MouseEvent) => {
    e.stopPropagation()
    if (d.childChainId) {
      d.onNavigateChild?.(d.childChainId)
    }
  }

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
          <GitBranch className={styles.icon} />
          {statusIcons[d.status] ?? statusIcons.pending}
          <span className={styles.label}>{d.label}</span>
          <button
            type="button"
            onClick={handleToggle}
            aria-label={d.expanded ? 'Collapse sub-chain' : 'Expand sub-chain'}
            className="ml-1 p-0.5 rounded hover:bg-gray-200/50"
          >
            {d.expanded
              ? <ChevronDown className="h-3.5 w-3.5 text-gray-500" />
              : <ChevronRight className="h-3.5 w-3.5 text-gray-500" />}
          </button>
        </div>
        <p className={styles.subLabel}>{d.subChainName}</p>
        {!d.expanded && d.childStepCount !== undefined && (
          <p className={styles.childCount}>{d.childStepCount} steps</p>
        )}
        {d.childChainId && (
          <button
            type="button"
            onClick={handleNavigate}
            className="mt-1 text-xs text-primary-400 hover:underline"
            aria-label={`Navigate to child chain ${d.childChainId.slice(0, 8)}`}
          >
            {d.childChainId.slice(0, 8)}...
          </button>
        )}
      </div>
      <Handle type="source" position={Position.Bottom} className={styles.handle} />
    </>
  )
})
