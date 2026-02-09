import { AreaChart, Area, XAxis, YAxis, Tooltip, ResponsiveContainer, Legend } from 'recharts'
import styles from './TimeSeriesChart.module.css'

interface Series {
  key: string
  color: string
  label: string
}

interface TimeSeriesChartProps {
  data: Record<string, unknown>[]
  series: Series[]
  xKey?: string
}

export function TimeSeriesChart({ data, series, xKey = 'time' }: TimeSeriesChartProps) {
  if (data.length === 0) {
    return (
      <div className={styles.emptyState}>
        No data available
      </div>
    )
  }

  return (
    <ResponsiveContainer width="100%" height={280}>
      <AreaChart data={data} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
        <defs>
          {series.map((s) => (
            <linearGradient key={s.key} id={`grad-${s.key}`} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={s.color} stopOpacity={0.2} />
              <stop offset="100%" stopColor={s.color} stopOpacity={0} />
            </linearGradient>
          ))}
        </defs>
        <XAxis
          dataKey={xKey}
          tick={{ fontSize: 11, fill: 'var(--color-gray-500)' }}
          axisLine={{ stroke: 'var(--color-gray-200)' }}
          tickLine={false}
        />
        <YAxis
          tick={{ fontSize: 11, fill: 'var(--color-gray-500)' }}
          axisLine={false}
          tickLine={false}
          width={40}
        />
        <Tooltip
          contentStyle={{
            backgroundColor: 'var(--color-gray-0)',
            border: '1px solid var(--color-gray-200)',
            borderRadius: '6px',
            fontSize: '13px',
          }}
        />
        <Legend
          wrapperStyle={{ fontSize: '12px', paddingTop: '8px' }}
        />
        {series.map((s) => (
          <Area
            key={s.key}
            type="monotone"
            dataKey={s.key}
            name={s.label}
            stroke={s.color}
            fill={`url(#grad-${s.key})`}
            strokeWidth={2}
            dot={false}
          />
        ))}
      </AreaChart>
    </ResponsiveContainer>
  )
}
