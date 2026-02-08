import { cn } from '../../lib/cn'
import styles from './Tabs.module.css'

interface Tab {
  id: string
  label: string
  count?: number
}

interface TabsProps {
  tabs: Tab[]
  active: string
  onChange: (id: string) => void
  size?: 'sm' | 'md'
}

export function Tabs({ tabs, active, onChange, size = 'md' }: TabsProps) {
  return (
    <div role="tablist" className={styles.tabList}>
      {tabs.map((tab) => (
        <button
          key={tab.id}
          role="tab"
          aria-selected={active === tab.id}
          onClick={() => onChange(tab.id)}
          className={cn(
            styles.tab,
            size === 'sm' ? styles.tabSm : styles.tabMd,
            active === tab.id ? styles.tabActive : styles.tabInactive,
          )}
        >
          {tab.label}
          {tab.count !== undefined && (
            <span className={styles.count}>
              {tab.count}
            </span>
          )}
        </button>
      ))}
    </div>
  )
}
