// Re-export from the Zustand toast store so existing consumers keep working.
import { useToastStore } from '../../stores/toast'
export type { Severity } from '../../stores/toast'

export function useToast() {
  const toast = useToastStore((s) => s.toast)
  return { toast }
}
