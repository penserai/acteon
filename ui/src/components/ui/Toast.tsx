import { createContext, useContext, useCallback, useState, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { CheckCircle2, XCircle, AlertTriangle, Info, X } from 'lucide-react'
import { cn } from '../../lib/cn'
import styles from './Toast.module.css'

type Severity = 'success' | 'error' | 'warning' | 'info'

interface Toast {
  id: number
  severity: Severity
  title: string
  description?: string
}

interface ToastContextType {
  toast: (severity: Severity, title: string, description?: string) => void
}

const ToastContext = createContext<ToastContextType>({ toast: () => {} })

export function useToast() {
  return useContext(ToastContext)
}

const icons: Record<Severity, ReactNode> = {
  success: <CheckCircle2 className={styles.iconSuccess} />,
  error: <XCircle className={styles.iconError} />,
  warning: <AlertTriangle className={styles.iconWarning} />,
  info: <Info className={styles.iconInfo} />,
}

const borders: Record<Severity, string> = {
  success: styles.success,
  error: styles.error,
  warning: styles.warning,
  info: styles.info,
}

let nextId = 0

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([])

  const addToast = useCallback((severity: Severity, title: string, description?: string) => {
    const id = nextId++
    setToasts((prev) => [...prev.slice(-4), { id, severity, title, description }])
    if (severity !== 'error') {
      setTimeout(() => setToasts((prev) => prev.filter((t) => t.id !== id)), 5000)
    }
  }, [])

  const dismiss = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id))
  }, [])

  return (
    <ToastContext.Provider value={{ toast: addToast }}>
      {children}
      <div className={styles.container} aria-live="polite">
        <AnimatePresence>
          {toasts.map((t) => (
            <motion.div
              key={t.id}
              role={t.severity === 'error' || t.severity === 'warning' ? 'alert' : 'status'}
              className={cn(
                styles.toast,
                borders[t.severity],
              )}
              initial={{ opacity: 0, x: 100 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: 100 }}
              transition={{ duration: 0.3, ease: [0.16, 1, 0.3, 1] }}
            >
              <span className={styles.icon}>{icons[t.severity]}</span>
              <div className={styles.content}>
                <p className={styles.title}>{t.title}</p>
                {t.description && (
                  <p className={styles.description}>{t.description}</p>
                )}
              </div>
              <button
                onClick={() => dismiss(t.id)}
                aria-label="Dismiss notification"
                className={styles.closeButton}
              >
                <X className={styles.closeIcon} />
              </button>
            </motion.div>
          ))}
        </AnimatePresence>
      </div>
    </ToastContext.Provider>
  )
}
