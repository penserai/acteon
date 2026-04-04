import type {
  Notebook,
  NotebookSummary,
  KernelInfo,
  PackageInfo,
  FileEntry,
  Language,
} from '../types/notebook'

const BASE_URL = import.meta.env.VITE_API_URL ?? ''

async function apiFetch<T>(path: string, options: RequestInit = {}): Promise<T> {
  const isFormData = options.body instanceof FormData
  const res = await fetch(`${BASE_URL}${path}`, {
    ...options,
    headers: {
      ...(isFormData ? {} : { 'Content-Type': 'application/json' }),
      ...options.headers,
    },
  })
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText)
    throw new Error(`${res.status}: ${text}`)
  }
  if (res.status === 204) return undefined as T
  return res.json() as Promise<T>
}

function apiGet<T>(path: string, params?: Record<string, string | number | undefined>): Promise<T> {
  const base = BASE_URL || window.location.origin
  const url = new URL(path, base)
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      if (v !== undefined) url.searchParams.set(k, String(v))
    }
  }
  return apiFetch<T>(url.pathname + url.search)
}

function apiPost<T>(path: string, body?: unknown): Promise<T> {
  return apiFetch<T>(path, {
    method: 'POST',
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
}

function apiPut<T>(path: string, body?: unknown): Promise<T> {
  return apiFetch<T>(path, {
    method: 'PUT',
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
}

function apiDelete<T>(path: string): Promise<T> {
  return apiFetch<T>(path, { method: 'DELETE' })
}

// ---- Notebooks ----

export interface NotebookListResponse {
  notebooks: NotebookSummary[]
}

export interface CreateNotebookResponse {
  id: string
}

export interface ExecuteCellResponse {
  queued: boolean
}

export function listNotebooks(): Promise<NotebookListResponse> {
  return apiGet<NotebookListResponse>('/api/notebooks')
}

export function createNotebook(name: string, language: Language): Promise<CreateNotebookResponse> {
  return apiPost<CreateNotebookResponse>('/api/notebooks', { name, language })
}

export function getNotebook(id: string): Promise<Notebook> {
  return apiGet<Notebook>(`/api/notebooks/${id}`)
}

export function updateNotebook(id: string, notebook: Partial<Notebook>): Promise<Notebook> {
  return apiPut<Notebook>(`/api/notebooks/${id}`, notebook)
}

export function deleteNotebook(id: string): Promise<void> {
  return apiDelete<void>(`/api/notebooks/${id}`)
}

export function executeCell(notebookId: string, cellId: string): Promise<ExecuteCellResponse> {
  return apiPost<ExecuteCellResponse>(`/api/notebooks/${notebookId}/cells/${cellId}/execute`)
}

// ---- Kernels ----

export interface KernelListResponse {
  kernels: KernelInfo[]
}

export function listKernels(): Promise<KernelListResponse> {
  return apiGet<KernelListResponse>('/api/kernels')
}

export function killKernel(id: string): Promise<void> {
  return apiDelete<void>(`/api/kernels/${id}`)
}

// ---- Packages ----

export interface PackageListResponse {
  packages: PackageInfo[]
}

export interface InstallPackageResponse {
  success: boolean
  message: string | null
}

export function listPackages(language: Language): Promise<PackageListResponse> {
  return apiGet<PackageListResponse>('/api/packages', { language })
}

export function installPackage(language: Language, name: string): Promise<InstallPackageResponse> {
  return apiPost<InstallPackageResponse>('/api/packages/install', { language, name })
}

// ---- Files ----

export interface FileListResponse {
  files: FileEntry[]
  path: string
}

export function listFiles(path?: string): Promise<FileListResponse> {
  return apiGet<FileListResponse>('/api/files', path ? { path } : undefined)
}
