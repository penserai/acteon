import shared from '../../styles/shared.module.css'

interface DetailGridProps {
  entries: Record<string, string> | [string, string][]
}

export function DetailGrid({ entries }: DetailGridProps) {
  const items = Array.isArray(entries) ? entries : Object.entries(entries)
  return (
    <div>
      {items.map(([k, v]) => (
        <div key={k} className={shared.detailRow}>
          <span className={shared.detailLabel}>{k}</span>
          <span className={shared.detailValueWrap}>{v}</span>
        </div>
      ))}
    </div>
  )
}
