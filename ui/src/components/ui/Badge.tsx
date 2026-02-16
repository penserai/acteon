import type { ReactNode } from 'react'
import { cn } from '../../lib/cn'
import styles from './Badge.module.css'

const variants = {
  success: styles.success,
  error: styles.error,
  warning: styles.warning,
  info: styles.info,
  pending: styles.pending,
  neutral: styles.neutral,
}

const sizes = {
  sm: styles.sm,
  md: styles.md,
}

const outcomeVariant: Record<string, keyof typeof variants> = {
  Executed: 'success',
  Deduplicated: 'neutral',
  Suppressed: 'error',
  Rerouted: 'info',
  Throttled: 'warning',
  Failed: 'error',
  Grouped: 'pending',
  PendingApproval: 'warning',
  ChainStarted: 'info',
  DryRun: 'neutral',
  CircuitOpen: 'error',
  Scheduled: 'pending',
  StateChanged: 'info',
  // Chain statuses
  running: 'info',
  completed: 'success',
  failed: 'error',
  cancelled: 'warning',
  timed_out: 'warning',
  waiting_sub_chain: 'info',
  pending: 'neutral',
  skipped: 'neutral',
  // Circuit states
  closed: 'success',
  open: 'error',
  half_open: 'warning',
  // Verdict
  allow: 'success',
  deny: 'error',
  // Approval
  approved: 'success',
  rejected: 'error',
  expired: 'warning',
  // Group states
  Pending: 'warning',
  Notified: 'info',
  Resolved: 'success',
  // Recurring action states
  Active: 'success',
  Paused: 'warning',
  Completed: 'neutral',
}

interface BadgeProps {
  variant?: keyof typeof variants
  size?: keyof typeof sizes
  children: ReactNode
  className?: string
}

export function Badge({ variant, size = 'sm', children, className }: BadgeProps) {
  const v = variant ?? outcomeVariant[children as string] ?? 'neutral'
  return (
    <span
      className={cn(
        styles.badge,
        variants[v],
        sizes[size],
        className,
      )}
    >
      {children}
    </span>
  )
}
