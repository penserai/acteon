import { useLocation } from 'react-router-dom'
import { Sun, Moon, Monitor, Command, Menu } from 'lucide-react'
import { useThemeStore } from '../../stores/theme'
import { useUiStore } from '../../stores/ui'
import styles from './Header.module.css'

const routeNames: Record<string, string> = {
  '/': 'Dashboard',
  '/dispatch': 'Dispatch',
  '/rules': 'Rules',
  '/audit': 'Audit Trail',
  '/events': 'Events',
  '/groups': 'Groups',
  '/chains': 'Chains',
  '/approvals': 'Approvals',
  '/circuit-breakers': 'Circuit Breakers',
  '/dlq': 'Dead-Letter Queue',
  '/stream': 'Event Stream',
  '/embeddings': 'Embeddings',
  '/settings': 'Settings',
  '/settings/rate-limiting': 'Rate Limiting',
  '/settings/auth': 'Auth & Users',
  '/settings/providers': 'Providers',
  '/settings/llm': 'LLM Guardrail',
  '/settings/telemetry': 'Telemetry',
  '/settings/config': 'Server Config',
  '/settings/background': 'Background Tasks',
}

export function Header() {
  const location = useLocation()
  const mode = useThemeStore((s) => s.mode)
  const cycleMode = useThemeStore((s) => s.cycleMode)
  const setCommandPaletteOpen = useUiStore((s) => s.setCommandPaletteOpen)
  const setMobileSidebarOpen = useUiStore((s) => s.setMobileSidebarOpen)

  const pathSegments = location.pathname.split('/').filter(Boolean)
  const breadcrumbs = pathSegments.reduce<{ path: string; label: string }[]>((acc, seg, i) => {
    const path = '/' + pathSegments.slice(0, i + 1).join('/')
    const label = routeNames[path] ?? seg
    acc.push({ path, label })
    return acc
  }, [])

  if (breadcrumbs.length === 0) {
    breadcrumbs.push({ path: '/', label: 'Dashboard' })
  }

  return (
    <header className={styles.header}>
      <div className={styles.leftSection}>
        <button
          onClick={() => setMobileSidebarOpen(true)}
          aria-label="Open menu"
          className={styles.menuButton}
        >
          <Menu className="h-5 w-5" />
        </button>
        <nav aria-label="Breadcrumb">
          <ol className={styles.breadcrumbNav}>
            {breadcrumbs.map((crumb, i) => (
              <li key={crumb.path} className={styles.breadcrumbItem}>
                {i > 0 && <span className={styles.breadcrumbSeparator}>/</span>}
                {i === breadcrumbs.length - 1 ? (
                  <span className={styles.breadcrumbCurrent} aria-current="page">
                    {crumb.label}
                  </span>
                ) : (
                  <span className={styles.breadcrumbLink}>{crumb.label}</span>
                )}
              </li>
            ))}
          </ol>
        </nav>
      </div>

      <div className={styles.rightSection}>
        <button
          onClick={cycleMode}
          aria-label={`Theme: ${mode}. Click to change.`}
          className={styles.themeButton}
        >
          {mode === 'light' ? <Sun className="h-4 w-4" /> :
           mode === 'dark' ? <Moon className="h-4 w-4" /> :
           <Monitor className="h-4 w-4" />}
        </button>

        <button
          onClick={() => setCommandPaletteOpen(true)}
          className={styles.commandButton}
        >
          <Command className="h-3 w-3" />
          <span>K</span>
        </button>
      </div>
    </header>
  )
}
