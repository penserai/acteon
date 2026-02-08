import { create } from 'zustand'

interface UiState {
  sidebarCollapsed: boolean
  mobileSidebarOpen: boolean
  commandPaletteOpen: boolean
  toggleSidebar: () => void
  setSidebarCollapsed: (collapsed: boolean) => void
  setMobileSidebarOpen: (open: boolean) => void
  setCommandPaletteOpen: (open: boolean) => void
}

const storedCollapsed = typeof localStorage !== 'undefined'
  ? localStorage.getItem('acteon-sidebar-collapsed') === 'true'
  : false

export const useUiStore = create<UiState>((set) => ({
  sidebarCollapsed: storedCollapsed,
  mobileSidebarOpen: false,
  commandPaletteOpen: false,
  toggleSidebar: () =>
    set((s) => {
      const next = !s.sidebarCollapsed
      localStorage.setItem('acteon-sidebar-collapsed', String(next))
      return { sidebarCollapsed: next }
    }),
  setSidebarCollapsed: (collapsed) => {
    localStorage.setItem('acteon-sidebar-collapsed', String(collapsed))
    set({ sidebarCollapsed: collapsed })
  },
  setMobileSidebarOpen: (open) => set({ mobileSidebarOpen: open }),
  setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),
}))
