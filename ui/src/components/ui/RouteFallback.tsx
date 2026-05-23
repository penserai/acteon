import styles from './RouteFallback.module.css'

/**
 * Loading indicator shown while a lazy-loaded route chunk is being
 * fetched. Kept intentionally small: a centered spinner with a
 * hidden "Loading page" label for screen readers.
 *
 * Rendered by the `<Suspense>` boundary inside `AppShell.tsx`
 * (wrapping the `<Outlet />`). Because the Suspense lives below
 * the Sidebar + Header in the component tree, this fallback only
 * replaces the main content area — the shell stays mounted and
 * navigation remains responsive while the page chunk loads.
 */
export function RouteFallback() {
  return (
    <div className={styles.container} role="status" aria-live="polite">
      <div className={styles.spinner} aria-hidden="true" />
      <span className={styles.srOnly}>Loading page</span>
    </div>
  )
}
