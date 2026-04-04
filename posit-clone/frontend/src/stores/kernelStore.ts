import { create } from 'zustand'
import type { KernelInfo, KernelStatus } from '../types/notebook'

interface KernelState {
  // State: kernelId -> KernelInfo
  kernels: Map<string, KernelInfo>
  // Cell IDs currently executing
  executing: Set<string>

  // Actions
  setKernelStatus: (kernelId: string, status: KernelStatus) => void
  setKernel: (kernel: KernelInfo) => void
  removeKernel: (kernelId: string) => void
  setExecuting: (cellId: string) => void
  clearExecuting: (cellId: string) => void
}

export const useKernelStore = create<KernelState>((set) => ({
  kernels: new Map(),
  executing: new Set(),

  setKernelStatus: (kernelId: string, status: KernelStatus) =>
    set((s) => {
      const kernel = s.kernels.get(kernelId)
      if (!kernel) return s
      const next = new Map(s.kernels)
      next.set(kernelId, { ...kernel, status })
      return { kernels: next }
    }),

  setKernel: (kernel: KernelInfo) =>
    set((s) => {
      const next = new Map(s.kernels)
      next.set(kernel.id, kernel)
      return { kernels: next }
    }),

  removeKernel: (kernelId: string) =>
    set((s) => {
      const next = new Map(s.kernels)
      next.delete(kernelId)
      return { kernels: next }
    }),

  setExecuting: (cellId: string) =>
    set((s) => {
      const next = new Set(s.executing)
      next.add(cellId)
      return { executing: next }
    }),

  clearExecuting: (cellId: string) =>
    set((s) => {
      const next = new Set(s.executing)
      next.delete(cellId)
      return { executing: next }
    }),
}))
