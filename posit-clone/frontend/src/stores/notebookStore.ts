import { create } from 'zustand'
import type { Notebook, NotebookSummary, Cell, CellType, CellOutput, Language } from '../types/notebook'
import { getNotebook, updateNotebook, listNotebooks } from '../api/client'

interface NotebookState {
  // State
  currentNotebook: Notebook | null
  notebooks: NotebookSummary[]
  loading: boolean
  error: string | null

  // Actions
  loadNotebooks: () => Promise<void>
  loadNotebook: (id: string) => Promise<void>
  saveNotebook: () => Promise<void>
  addCell: (type: CellType, language: Language, afterIndex?: number) => void
  deleteCell: (cellId: string) => void
  moveCell: (cellId: string, direction: 'up' | 'down') => void
  updateCellSource: (cellId: string, source: string) => void
  setCellOutputs: (cellId: string, outputs: CellOutput[]) => void
  setCellExecutionCount: (cellId: string, count: number) => void
}

function buildNewCell(type: CellType, language: Language): Cell {
  return {
    id: crypto.randomUUID(),
    cell_type: type,
    source: '',
    language,
    outputs: [],
    execution_count: null,
    metadata: {},
  }
}

export const useNotebookStore = create<NotebookState>((set, get) => ({
  currentNotebook: null,
  notebooks: [],
  loading: false,
  error: null,

  loadNotebooks: async () => {
    set({ loading: true, error: null })
    try {
      const res = await listNotebooks()
      set({ notebooks: res.notebooks, loading: false })
    } catch (err) {
      set({ error: String(err), loading: false })
    }
  },

  loadNotebook: async (id: string) => {
    set({ loading: true, error: null })
    try {
      const notebook = await getNotebook(id)
      set({ currentNotebook: notebook, loading: false })
    } catch (err) {
      set({ error: String(err), loading: false })
    }
  },

  saveNotebook: async () => {
    const { currentNotebook } = get()
    if (!currentNotebook) return
    set({ loading: true, error: null })
    try {
      const updated = await updateNotebook(currentNotebook.id, currentNotebook)
      set({ currentNotebook: updated, loading: false })
    } catch (err) {
      set({ error: String(err), loading: false })
    }
  },

  addCell: (type: CellType, language: Language, afterIndex?: number) => {
    const { currentNotebook } = get()
    if (!currentNotebook) return
    const cell = buildNewCell(type, language)
    const cells = [...currentNotebook.cells]
    const insertAt = afterIndex !== undefined ? afterIndex + 1 : cells.length
    cells.splice(insertAt, 0, cell)
    set({ currentNotebook: { ...currentNotebook, cells } })
  },

  deleteCell: (cellId: string) => {
    const { currentNotebook } = get()
    if (!currentNotebook) return
    set({
      currentNotebook: {
        ...currentNotebook,
        cells: currentNotebook.cells.filter((c) => c.id !== cellId),
      },
    })
  },

  moveCell: (cellId: string, direction: 'up' | 'down') => {
    const { currentNotebook } = get()
    if (!currentNotebook) return
    const cells = [...currentNotebook.cells]
    const idx = cells.findIndex((c) => c.id === cellId)
    if (idx === -1) return
    const swapIdx = direction === 'up' ? idx - 1 : idx + 1
    if (swapIdx < 0 || swapIdx >= cells.length) return
    const temp = cells[idx]
    cells[idx] = cells[swapIdx]
    cells[swapIdx] = temp
    set({ currentNotebook: { ...currentNotebook, cells } })
  },

  updateCellSource: (cellId: string, source: string) => {
    const { currentNotebook } = get()
    if (!currentNotebook) return
    set({
      currentNotebook: {
        ...currentNotebook,
        cells: currentNotebook.cells.map((c) =>
          c.id === cellId ? { ...c, source } : c,
        ),
      },
    })
  },

  setCellOutputs: (cellId: string, outputs: CellOutput[]) => {
    const { currentNotebook } = get()
    if (!currentNotebook) return
    set({
      currentNotebook: {
        ...currentNotebook,
        cells: currentNotebook.cells.map((c) =>
          c.id === cellId ? { ...c, outputs } : c,
        ),
      },
    })
  },

  setCellExecutionCount: (cellId: string, count: number) => {
    const { currentNotebook } = get()
    if (!currentNotebook) return
    set({
      currentNotebook: {
        ...currentNotebook,
        cells: currentNotebook.cells.map((c) =>
          c.id === cellId ? { ...c, execution_count: count } : c,
        ),
      },
    })
  },
}))
