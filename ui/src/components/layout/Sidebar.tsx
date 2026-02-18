import { useEffect } from 'react'
import { NavLink, useLocation } from 'react-router-dom'
import { cn } from '../../lib/cn'
import { useUiStore } from '../../stores/ui'
import {
  LayoutDashboard, Send, BookOpen, FlaskConical, ScrollText, Radio, Layers, Link2, ShieldCheck,
  Zap, AlertTriangle, Rss, Brain, Gauge, Users, Server, Cpu, Eye, Settings,
  PanelLeftClose, PanelLeftOpen, X, ExternalLink, RefreshCw, PieChart, Database, HeartPulse, Puzzle,
  ShieldAlert,
} from 'lucide-react'
import logoPng from '../../assets/logo.png'
import styles from './Sidebar.module.css'

const mainItems = [
  { to: '/', icon: LayoutDashboard, label: 'Dashboard' },
  { to: '/dispatch', icon: Send, label: 'Dispatch' },
  { to: '/rules', icon: BookOpen, label: 'Rules' },
  { to: '/playground', icon: FlaskConical, label: 'Rule Playground' },
  { to: '/audit', icon: ScrollText, label: 'Audit Trail' },
  { to: '/events', icon: Radio, label: 'Events' },
  { to: '/groups', icon: Layers, label: 'Groups' },
  { to: '/chains', icon: Link2, label: 'Chains' },
  { to: '/approvals', icon: ShieldCheck, label: 'Approvals', badge: true },
  { to: '/circuit-breakers', icon: Zap, label: 'Circuit Breakers' },
  { to: '/provider-health', icon: HeartPulse, label: 'Provider Health' },
  { to: '/dlq', icon: AlertTriangle, label: 'Dead-Letter Queue' },
  { to: '/stream', icon: Rss, label: 'Stream' },
  { to: '/embeddings', icon: Brain, label: 'Embeddings' },
  { to: '/recurring', icon: RefreshCw, label: 'Recurring Actions' },
  { to: '/quotas', icon: PieChart, label: 'Quotas' },
  { to: '/retention', icon: Database, label: 'Retention' },
  { to: '/wasm-plugins', icon: Puzzle, label: 'WASM Plugins' },
  { to: '/compliance', icon: ShieldAlert, label: 'Compliance' },
  {
    to: 'https://penserai.github.io/acteon',
    icon: ExternalLink,
    label: 'Documentation',
    external: true,
  },
]

const settingsItems = [
  { to: '/settings/rate-limiting', icon: Gauge, label: 'Rate Limiting' },
  { to: '/settings/auth', icon: Users, label: 'Auth & Users' },
  { to: '/settings/providers', icon: Server, label: 'Providers' },
  { to: '/settings/llm', icon: Cpu, label: 'LLM Guardrail' },
  { to: '/settings/telemetry', icon: Eye, label: 'Telemetry' },
  { to: '/settings/config', icon: Settings, label: 'Server Config' },
  { to: '/settings/background', icon: Cpu, label: 'Background Tasks' },
]

export function Sidebar() {
  const collapsed = useUiStore((s) => s.sidebarCollapsed)
  const toggle = useUiStore((s) => s.toggleSidebar)
  const mobileSidebarOpen = useUiStore((s) => s.mobileSidebarOpen)
  const setMobileSidebarOpen = useUiStore((s) => s.setMobileSidebarOpen)
  const location = useLocation()

  // Close mobile sidebar on navigation
  useEffect(() => {
    setMobileSidebarOpen(false)
  }, [location.pathname, setMobileSidebarOpen])

  const sidebarContent = (
    <>
      <div className={styles.header}>
        <div className={styles.logoWrap}>
          <img src={logoPng} alt="Acteon" className={styles.logoImg} />
          <span className={styles.logo}>Acteon</span>
        </div>
        {/* Desktop: collapse toggle */}
        <button
          onClick={toggle}
          aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
          className={cn(styles.toggleButton, styles.toggleButtonDesktop)}
        >
          {collapsed ? <PanelLeftOpen className="h-4 w-4" /> : <PanelLeftClose className="h-4 w-4" />}
        </button>
        {/* Mobile: close button */}
        <button
          onClick={() => setMobileSidebarOpen(false)}
          aria-label="Close sidebar"
          className={cn(styles.toggleButton, styles.toggleButtonMobile)}
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      <div className={styles.scrollContainer}>
        <SidebarSection items={mainItems} collapsed={false} />
        <div className={styles.settingsHeader}>
          <span className={styles.settingsLabel}>Settings</span>
        </div>
        <SidebarSection items={settingsItems} collapsed={false} />
      </div>
    </>
  )

  return (
    <>
      {/* Desktop sidebar */}
      <nav
        aria-label="Main navigation"
        className={cn(
          styles.desktopNav,
          collapsed ? styles.desktopNavCollapsed : styles.desktopNavExpanded,
        )}
      >
        <div className={styles.header}>
          <div className={styles.logoWrap}>
            <img src={logoPng} alt="Acteon" className={styles.logoImg} />
            {!collapsed && <span className={styles.logo}>Acteon</span>}
          </div>
          <button
            onClick={toggle}
            aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
            className={styles.toggleButton}
          >
            {collapsed ? <PanelLeftOpen className="h-4 w-4" /> : <PanelLeftClose className="h-4 w-4" />}
          </button>
        </div>

        <div className={styles.scrollContainer}>
          <SidebarSection items={mainItems} collapsed={collapsed} />
          {!collapsed && (
            <div className={styles.settingsHeader}>
              <span className={styles.settingsLabel}>Settings</span>
            </div>
          )}
          {collapsed && <div className={styles.settingsDivider} />}
          <SidebarSection items={settingsItems} collapsed={collapsed} />
        </div>
      </nav>

      {/* Mobile sidebar overlay */}
      {mobileSidebarOpen && (
        <div className={styles.mobileOverlay}>
          {/* Backdrop */}
          <div
            className={styles.backdrop}
            onClick={() => setMobileSidebarOpen(false)}
            aria-hidden="true"
          />
          {/* Drawer */}
          <nav
            aria-label="Main navigation"
            className={styles.mobileNav}
          >
            {sidebarContent}
          </nav>
        </div>
      )}
    </>
  )
}

function SidebarSection({ items, collapsed }: { items: (typeof mainItems[0] & { external?: boolean })[]; collapsed: boolean }) {
  return (
    <ul className={styles.sectionList}>
      {items.map((item) => (
        <li key={item.to}>
          {item.external ? (
            <a
              href={item.to}
              target="_blank"
              rel="noopener noreferrer"
              className={cn(
                styles.navLink,
                collapsed ? styles.navLinkCollapsed : styles.navLinkExpanded,
                styles.navLinkInactive,
              )}
              title={collapsed ? item.label : undefined}
            >
              <item.icon className={styles.navIcon} />
              {!collapsed && <span className={styles.navLabel}>{item.label}</span>}
            </a>
          ) : (
            <NavLink
              to={item.to}
              end={item.to === '/'}
              className={({ isActive }) =>
                cn(
                  styles.navLink,
                  collapsed ? styles.navLinkCollapsed : styles.navLinkExpanded,
                  isActive ? styles.navLinkActive : styles.navLinkInactive,
                )
              }
              title={collapsed ? item.label : undefined}
            >
              {({ isActive }) => (
                <>
                  <item.icon className={cn(styles.navIcon, isActive && styles.navIconActive)} />
                  {!collapsed && <span className={styles.navLabel}>{item.label}</span>}
                </>
              )}
            </NavLink>
          )}
        </li>
      ))}
    </ul>
  )
}
