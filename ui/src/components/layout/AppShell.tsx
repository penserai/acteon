import { Suspense } from 'react'
import { Outlet, useLocation } from 'react-router-dom'
import { motion, AnimatePresence } from 'framer-motion'
import { Sidebar } from './Sidebar'
import { Header } from './Header'
import { RouteFallback } from '../ui/RouteFallback'
import styles from './AppShell.module.css'

export function AppShell() {
  const location = useLocation()

  return (
    <div className={styles.container}>
      <Sidebar />
      <div className={styles.content}>
        <Header />
        <main className={styles.main}>
          <AnimatePresence mode="wait">
            <motion.div
              key={location.pathname}
              initial={{ opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -4 }}
              transition={{ duration: 0.15, ease: [0.16, 1, 0.3, 1] }}
            >
              {/*
                Suspense lives inside the motion.div (and inside
                AppShell, below the Sidebar + Header) so that when a
                lazy-loaded route chunk is in flight:

                - The Sidebar and Header stay mounted and interactive
                  (nav links remain clickable, the command palette
                  still works).
                - AnimatePresence's exit animation on the previous
                  page runs to completion before the fallback
                  appears — the transition stays smooth instead of
                  popping mid-flight.
                - RouteFallback only replaces the main content area
                  rather than unmounting the whole shell.
              */}
              <Suspense fallback={<RouteFallback />}>
                <Outlet />
              </Suspense>
            </motion.div>
          </AnimatePresence>
        </main>
      </div>
    </div>
  )
}
