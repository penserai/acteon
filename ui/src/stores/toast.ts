import { create } from 'zustand'

export type Severity = 'success' | 'error' | 'warning' | 'info'

export interface Toast {
  id: number
  severity: Severity
  title: string
  description?: string
}

interface ToastState {
  toasts: Toast[]
  toast: (severity: Severity, title: string, description?: string) => void
  dismiss: (id: number) => void
}

let nextId = 0

export const useToastStore = create<ToastState>((set) => ({
  toasts: [],
  toast: (severity, title, description) => {
    const id = nextId++
    set((s) => ({
      toasts: [...s.toasts.slice(-4), { id, severity, title, description }]
    }))

    if (severity !== 'error') {
      setTimeout(() => {
        set((s) => ({
          toasts: s.toasts.filter((t) => t.id !== id)
        }))
      }, 5000)
    }
  },
  dismiss: (id) => set((s) => ({
    toasts: s.toasts.filter((t) => t.id !== id)
  })),
}))
