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

// IMPORTANT: keys must match the EXACT strings the server emits. Action
// outcomes and most status enums serialize as snake_case (e.g. the gateway's
// `outcome_tag`, `GroupState`'s `rename_all = "snake_case"`), so the keys here
// are snake_case too. Previously the action-outcome keys were PascalCase, so
// every outcome badge fell through to `neutral` (grey) and the outcome filter
// matched nothing.
const outcomeVariant: Record<string, keyof typeof variants> = {
  // Action outcomes (gateway `outcome_tag`, snake_case)
  executed: 'success',
  deduplicated: 'neutral',
  suppressed: 'error',
  silenced: 'info',
  muted: 'neutral',
  rerouted: 'info',
  throttled: 'warning',
  failed: 'error',
  grouped: 'pending',
  pending_approval: 'warning',
  chain_started: 'info',
  dry_run: 'neutral',
  circuit_open: 'error',
  scheduled: 'pending',
  state_changed: 'info',
  recurring_created: 'info',
  quota_exceeded: 'warning',
  // Compliance two-phase: a pre-execution intent record (outcome `pending`,
  // relabelled `Audit Intent` for display) — surfaced distinctly so it isn't
  // mistaken for a stuck/in-flight job.
  'Audit Intent': 'info',
  // Chain statuses
  running: 'info',
  completed: 'success',
  cancelled: 'warning',
  timed_out: 'warning',
  waiting_sub_chain: 'info',
  waiting_parallel: 'info',
  waiting_timer: 'info',
  waiting_signal: 'info',
  waiting_worker: 'info',
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
  // Event-group states (`GroupState`, snake_case: pending/notified/resolved —
  // `pending` shares the neutral entry above)
  notified: 'info',
  resolved: 'success',
  // Recurring-action states. The status string is UI-derived (PascalCase); the
  // snake_case aliases are kept too in case it is ever surfaced server-side.
  Active: 'success',
  Paused: 'warning',
  Completed: 'neutral',
  active: 'success',
  paused: 'warning',
  // Swarm run statuses (snake_case, as the server emits them). `running`,
  // `completed`, `failed`, `cancelled`, `timed_out` reuse the chain entries.
  accepted: 'pending',
  adversarial: 'info',
  cancelling: 'warning',
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
