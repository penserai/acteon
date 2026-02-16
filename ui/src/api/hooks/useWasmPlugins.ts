import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiDelete, apiFetch } from '../client'
import type {
  WasmPlugin,
  WasmPluginListResponse,
  WasmTestRequest,
  WasmTestResponse,
} from '../../types'

export function useWasmPlugins() {
  return useQuery({
    queryKey: ['wasm-plugins'],
    queryFn: () => apiGet<WasmPluginListResponse>('/v1/wasm/plugins'),
    refetchInterval: 15000,
  })
}

export function useWasmPlugin(name: string | undefined) {
  return useQuery({
    queryKey: ['wasm-plugins', name],
    queryFn: () => apiGet<WasmPlugin>(`/v1/wasm/plugins/${name}`),
    enabled: !!name,
  })
}

export function useRegisterWasmPlugin() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: async (formData: FormData) => {
      return apiFetch<WasmPlugin>('/v1/wasm/plugins', {
        method: 'POST',
        headers: {},  // Let browser set multipart boundary
        body: formData,
      })
    },
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['wasm-plugins'] }),
  })
}

export function useDeleteWasmPlugin() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (name: string) => apiDelete(`/v1/wasm/plugins/${name}`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['wasm-plugins'] }),
  })
}

export function useTestWasmPlugin() {
  return useMutation({
    mutationFn: ({ name, body }: { name: string; body: WasmTestRequest }) =>
      apiFetch<WasmTestResponse>(`/v1/wasm/plugins/${name}/test`, {
        method: 'POST',
        body: JSON.stringify(body),
      }),
  })
}
