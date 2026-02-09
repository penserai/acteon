import { cn } from '../../lib/cn'
import { formatNumber } from '../../lib/format'
import { TrendingUp, TrendingDown } from 'lucide-react'
import styles from './StatCard.module.css'

interface StatCardProps {
  label: string
  value: number
  trend?: number
  color?: string
  onClick?: () => void
  sparkData?: number[]
}

export function StatCard({ label, value, trend, onClick, sparkData }: StatCardProps) {
  return (
    <button
      onClick={onClick}
      className={cn(
        styles.card,
        onClick && styles.cardClickable,
      )}
    >
      <p className={styles.label}>{label}</p>
      <p className={styles.value}>
        {formatNumber(value)}
      </p>
      {sparkData && sparkData.length > 1 && (
        <Sparkline data={sparkData} className={styles.sparkline} />
      )}
      {trend !== undefined && (
        <div className={cn(
          styles.trend,
          trend >= 0 ? styles.trendPositive : styles.trendNegative,
        )}>
          {trend >= 0 ? <TrendingUp className="h-3 w-3" /> : <TrendingDown className="h-3 w-3" />}
          <span>{trend >= 0 ? '+' : ''}{trend.toFixed(1)}%</span>
        </div>
      )}
    </button>
  )
}

function Sparkline({ data, className }: { data: number[]; className?: string }) {
  const max = Math.max(...data, 1)
  const min = Math.min(...data, 0)
  const range = max - min || 1
  const w = 120
  const h = 24
  const points = data.map((v, i) => {
    const x = (i / (data.length - 1)) * w
    const y = h - ((v - min) / range) * h
    return `${x},${y}`
  }).join(' ')

  return (
    <svg width={w} height={h} className={className} viewBox={`0 0 ${w} ${h}`}>
      <polyline
        points={points}
        fill="none"
        stroke="var(--color-primary-400)"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  )
}
