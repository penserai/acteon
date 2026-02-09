import { create } from 'zustand'

type ThemeMode = 'system' | 'light' | 'dark'

interface ThemeState {
  mode: ThemeMode
  setMode: (mode: ThemeMode) => void
  cycleMode: () => void
  isDark: () => boolean
}

function applyTheme(mode: ThemeMode) {
  const root = document.documentElement
  if (mode === 'dark' || (mode === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)) {
    root.classList.add('dark')
  } else {
    root.classList.remove('dark')
  }
}

const stored = (typeof localStorage !== 'undefined' ? localStorage.getItem('acteon-theme') : null) as ThemeMode | null

export const useThemeStore = create<ThemeState>((set, get) => {
  const initial = stored ?? 'system'
  // Apply on init
  if (typeof document !== 'undefined') applyTheme(initial)

  return {
    mode: initial,
    setMode: (mode) => {
      localStorage.setItem('acteon-theme', mode)
      applyTheme(mode)
      set({ mode })
    },
    cycleMode: () => {
      const order: ThemeMode[] = ['system', 'light', 'dark']
      const current = get().mode
      const next = order[(order.indexOf(current) + 1) % order.length]
      get().setMode(next)
    },
    isDark: () => {
      const { mode } = get()
      if (mode === 'dark') return true
      if (mode === 'light') return false
      return typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches
    },
  }
})
