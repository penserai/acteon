// ---- Enumerations ----

export type CellType = 'code' | 'markdown' | 'raw'

export type Language = 'python' | 'r' | 'julia' | 'javascript' | 'typescript'

export type OutputType =
  | 'stream'
  | 'display_data'
  | 'execute_result'
  | 'error'

export type KernelStatus =
  | 'starting'
  | 'idle'
  | 'busy'
  | 'restarting'
  | 'dead'
  | 'unknown'

// ---- Cell Output ----

export interface StreamOutput {
  output_type: 'stream'
  name: 'stdout' | 'stderr'
  text: string[]
}

export interface DisplayDataOutput {
  output_type: 'display_data'
  data: Record<string, string>
  metadata: Record<string, unknown>
}

export interface ExecuteResultOutput {
  output_type: 'execute_result'
  execution_count: number
  data: Record<string, string>
  metadata: Record<string, unknown>
}

export interface ErrorOutput {
  output_type: 'error'
  ename: string
  evalue: string
  traceback: string[]
}

export type CellOutput =
  | StreamOutput
  | DisplayDataOutput
  | ExecuteResultOutput
  | ErrorOutput

// ---- Cell ----

export interface Cell {
  id: string
  cell_type: CellType
  source: string
  language: Language
  outputs: CellOutput[]
  execution_count: number | null
  metadata: Record<string, unknown>
}

// ---- Notebook ----

export interface Notebook {
  id: string
  name: string
  language: Language
  cells: Cell[]
  metadata: Record<string, unknown>
  created_at: string
  updated_at: string
}

export interface NotebookSummary {
  id: string
  name: string
  language: Language
  cell_count: number
  created_at: string
  updated_at: string
}

// ---- Kernel ----

export interface KernelInfo {
  id: string
  notebook_id: string
  language: Language
  status: KernelStatus
  started_at: string | null
  last_activity_at: string | null
}

// ---- Package ----

export interface PackageInfo {
  name: string
  version: string | null
  description: string | null
  installed: boolean
}

// ---- File System ----

export interface FileEntry {
  name: string
  path: string
  is_dir: boolean
  size: number | null
  modified_at: string | null
}

// ---- WebSocket message types ----

export type WsMessageType =
  | 'execution_output'
  | 'execution_complete'
  | 'kernel_status'
  | 'error'

export interface ExecutionOutputMsg {
  type: 'execution_output'
  cell_id: string
  output: CellOutput
}

export interface ExecutionCompleteMsg {
  type: 'execution_complete'
  cell_id: string
  execution_count: number
}

export interface KernelStatusMsg {
  type: 'kernel_status'
  status: KernelStatus
}

export interface WsErrorMsg {
  type: 'error'
  message: string
}

export type WsMessage =
  | ExecutionOutputMsg
  | ExecutionCompleteMsg
  | KernelStatusMsg
  | WsErrorMsg
