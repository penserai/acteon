import { useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { Command } from 'cmdk'
import { AnimatePresence, motion } from 'framer-motion'
import {
  LayoutDashboard, Send, BookOpen, ScrollText, Link2, ShieldCheck, Rss,
  Sun, Search,
} from 'lucide-react'
import { useUiStore } from '../../stores/ui'
import { useThemeStore } from '../../stores/theme'
import styles from './CommandPalette.module.css'

const navItems = [
  { label: 'Dashboard', to: '/', icon: LayoutDashboard, shortcut: '1' },
  { label: 'Rules', to: '/rules', icon: BookOpen, shortcut: '2' },
  { label: 'Chains', to: '/chains', icon: Link2, shortcut: '3' },
  { label: 'Audit Trail', to: '/audit', icon: ScrollText, shortcut: '4' },
  { label: 'Approvals', to: '/approvals', icon: ShieldCheck, shortcut: '5' },
  { label: 'Event Stream', to: '/stream', icon: Rss },
  { label: 'Dispatch Action', to: '/dispatch', icon: Send },
]

export function CommandPalette() {
  const open = useUiStore((s) => s.commandPaletteOpen)
  const setOpen = useUiStore((s) => s.setCommandPaletteOpen)
  const cycleMode = useThemeStore((s) => s.cycleMode)
  const navigate = useNavigate()

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'k' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault()
        setOpen(!open)
      }
      if (e.key === 'Escape' && open) {
        e.preventDefault()
        setOpen(false)
      }
    }
    document.addEventListener('keydown', handler)
    return () => document.removeEventListener('keydown', handler)
  }, [open, setOpen])

  const goTo = (path: string) => {
    navigate(path)
    setOpen(false)
  }

  return (
    <AnimatePresence>
      {open && (
        <div className={styles.overlay}>
          <motion.div
            className={styles.backdrop}
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            onClick={() => setOpen(false)}
          />
          <motion.div
            className={styles.dialog}
            initial={{ opacity: 0, scale: 0.95, y: -8 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.95 }}
            transition={{ duration: 0.2, ease: [0.34, 1.56, 0.64, 1] }}
          >
            <Command label="Command palette">
              <div className={styles.inputWrapper}>
                <Search className={styles.searchIcon} />
                <Command.Input
                  placeholder="Type a command or search..."
                  className={styles.input}
                  autoFocus
                />
              </div>
              <Command.List className={styles.list}>
                <Command.Empty className={styles.empty}>
                  No results found.
                </Command.Empty>

                <Command.Group heading="Navigation" className={styles.group}>
                  {navItems.map((item) => (
                    <Command.Item
                      key={item.to}
                      onSelect={() => goTo(item.to)}
                      className={styles.item}
                    >
                      <item.icon className={styles.itemIcon} />
                      <span className={styles.itemLabel}>{item.label}</span>
                      {item.shortcut && (
                        <kbd className={styles.shortcut}>Cmd+{item.shortcut}</kbd>
                      )}
                    </Command.Item>
                  ))}
                </Command.Group>

                <Command.Group heading="Actions" className={styles.group}>
                  <Command.Item
                    onSelect={() => { cycleMode(); setOpen(false) }}
                    className={styles.item}
                  >
                    <Sun className={styles.itemIcon} />
                    <span className={styles.itemLabel}>Toggle Theme</span>
                  </Command.Item>
                </Command.Group>
              </Command.List>
            </Command>
          </motion.div>
        </div>
      )}
    </AnimatePresence>
  )
}
