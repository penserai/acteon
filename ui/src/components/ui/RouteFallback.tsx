import styles from './RouteFallback.module.css'

/**
 * Loading indicator shown while a lazy-loaded route chunk is being
 * fetched. Kept intentionally small: a centered spinner with a
 * hidden "Loading page" label for screen readers.
 *
 * Used as the `<Suspense fallback={...}>` for every route in
 * `App.tsx`. Because the eager bundle already carries the AppShell
 * (sidebar + header), this fallback only fills the main content
 * area — navigation stays responsive while the page chunk loads.
 */
export function RouteFallback() {
  return (
    <div className={styles.container} role="status" aria-live="polite">
      <div className={styles.spinner} aria-hidden="true" />
      <span className={styles.srOnly}>Loading page</span>
    </div>
  )
}
