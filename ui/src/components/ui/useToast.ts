import { createContext, useContext } from 'react'

export type Severity = 'success' | 'error' | 'warning' | 'info'

export interface ToastContextType {
  toast: (severity: Severity, title: string, description?: string) => void
}

export const ToastContext = createContext<ToastContextType>({ toast: () => {} })

export function useToast() {
  return useContext(ToastContext)
}
