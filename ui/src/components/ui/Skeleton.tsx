import { cn } from '../../lib/cn'
import styles from './Skeleton.module.css'

interface SkeletonProps {
  className?: string
}

export function Skeleton({ className }: SkeletonProps) {
  return (
    <div className={cn(styles.skeleton, className)} />
  )
}

export function TableSkeleton({ rows = 5, cols = 5 }: { rows?: number; cols?: number }) {
  return (
    <div className={styles.tableWrapper}>
      {Array.from({ length: rows }).map((_, i) => (
        <div key={i} className={styles.tableRow}>
          {Array.from({ length: cols }).map((_, j) => (
            <Skeleton key={j} className={styles.tableCell} />
          ))}
        </div>
      ))}
    </div>
  )
}

export function StatCardSkeleton() {
  return (
    <div className={styles.statCard}>
      <Skeleton className={styles.statTitle} />
      <Skeleton className={styles.statValue} />
      <Skeleton className={styles.statFull} />
    </div>
  )
}
