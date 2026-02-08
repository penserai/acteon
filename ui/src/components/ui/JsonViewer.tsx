import { useState } from 'react'
import { Copy, Check, ChevronRight, ChevronDown } from 'lucide-react'
import { cn } from '../../lib/cn'
import styles from './JsonViewer.module.css'

interface JsonViewerProps {
  data: unknown
  collapsed?: boolean
  className?: string
}

export function JsonViewer({ data, collapsed = false, className }: JsonViewerProps) {
  const [copied, setCopied] = useState(false)
  const text = JSON.stringify(data, null, 2)

  const handleCopy = () => {
    void navigator.clipboard.writeText(text)
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }

  return (
    <div className={cn(styles.wrapper, 'group', className)}>
      <button
        onClick={handleCopy}
        aria-label="Copy JSON"
        className={cn(styles.copyButton, 'group-hover:opacity-100')}
      >
        {copied ? <Check className={styles.copyIcon} /> : <Copy className={styles.copyIcon} />}
      </button>
      <JsonNode data={data} depth={0} defaultCollapsed={collapsed} />
    </div>
  )
}

function JsonNode({ data, depth, defaultCollapsed }: { data: unknown; depth: number; defaultCollapsed: boolean }) {
  const [collapsed, setCollapsed] = useState(defaultCollapsed && depth > 0)
  const indent = depth * 16

  if (data === null) return <span className={styles.null}>null</span>
  if (typeof data === 'boolean') return <span className={styles.boolean}>{String(data)}</span>
  if (typeof data === 'number') return <span className={styles.number}>{data}</span>
  if (typeof data === 'string') return <span className={styles.string}>&quot;{data}&quot;</span>

  if (Array.isArray(data)) {
    if (data.length === 0) return <span className={styles.emptyArray}>[]</span>
    return (
      <div>
        <button
          onClick={() => setCollapsed(!collapsed)}
          className={styles.toggleButton}
        >
          {collapsed ? <ChevronRight className={styles.chevron} /> : <ChevronDown className={styles.chevron} />}
          <span className={styles.bracketMono}>[{collapsed ? `${data.length} items` : ''}]</span>
        </button>
        {!collapsed && (
          <div style={{ paddingLeft: indent + 16 }}>
            {data.map((item, i) => (
              <div key={i} className={styles.item}>
                <JsonNode data={item} depth={depth + 1} defaultCollapsed={defaultCollapsed} />
                {i < data.length - 1 && <span className={styles.comma}>,</span>}
              </div>
            ))}
          </div>
        )}
        {!collapsed && <span className={styles.emptyArray}>]</span>}
      </div>
    )
  }

  if (typeof data === 'object') {
    const entries = Object.entries(data as Record<string, unknown>)
    if (entries.length === 0) return <span className={styles.emptyObject}>{'{}'}</span>
    return (
      <div>
        <button
          onClick={() => setCollapsed(!collapsed)}
          className={styles.toggleButton}
        >
          {collapsed ? <ChevronRight className={styles.chevron} /> : <ChevronDown className={styles.chevron} />}
          <span className={styles.bracketMono}>{'{'}{collapsed ? `${entries.length} keys` : ''}</span>
        </button>
        {!collapsed && (
          <div style={{ paddingLeft: indent + 16 }}>
            {entries.map(([key, val], i) => (
              <div key={key} className={styles.item}>
                <span className={styles.key}>&quot;{key}&quot;</span>
                <span className={styles.colon}>: </span>
                <JsonNode data={val} depth={depth + 1} defaultCollapsed={defaultCollapsed} />
                {i < entries.length - 1 && <span className={styles.comma}>,</span>}
              </div>
            ))}
          </div>
        )}
        {!collapsed && <span className={styles.emptyObject}>{'}'}</span>}
      </div>
    )
  }

  return <span className={styles.fallback}>{String(data)}</span>
}
