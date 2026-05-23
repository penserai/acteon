import { useEffect } from 'react'
import { NavLink, useLocation } from 'react-router-dom'
import { cn } from '../../lib/cn'
import { useUiStore } from '../../stores/ui'
import { MAIN_NAV_ITEMS, SETTINGS_NAV_ITEMS, type NavItem } from '../../lib/navigation'
import {
  PanelLeftClose, PanelLeftOpen, X,
} from 'lucide-react'
import logoSvg from '../../assets/logo.svg'
import styles from './Sidebar.module.css'

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
          <img src={logoSvg} alt="Acteon" className={styles.logoImg} />
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
        <SidebarSection items={MAIN_NAV_ITEMS} collapsed={false} />
        <div className={styles.settingsHeader}>
          <span className={styles.settingsLabel}>Settings</span>
        </div>
        <SidebarSection items={SETTINGS_NAV_ITEMS} collapsed={false} />
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
            <img src={logoSvg} alt="Acteon" className={styles.logoImg} />
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
          <SidebarSection items={MAIN_NAV_ITEMS} collapsed={collapsed} />
          {!collapsed && (
            <div className={styles.settingsHeader}>
              <span className={styles.settingsLabel}>Settings</span>
            </div>
          )}
          {collapsed && <div className={styles.settingsDivider} />}
          <SidebarSection items={SETTINGS_NAV_ITEMS} collapsed={collapsed} />
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

function SidebarSection({ items, collapsed }: { items: NavItem[]; collapsed: boolean }) {
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
              end={item.end}
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
