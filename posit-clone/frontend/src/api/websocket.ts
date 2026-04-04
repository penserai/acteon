import type { WsMessage } from '../types/notebook'

const BASE_URL = import.meta.env.VITE_API_URL ?? ''

const MAX_RETRIES = 5
const BASE_BACKOFF_MS = 500

type MessageCallback = (msg: WsMessage) => void
type StatusCallback = (status: 'connecting' | 'connected' | 'disconnected') => void

export interface NotebookWebSocket {
  sendExecute: (cellId: string) => void
  onMessage: (cb: MessageCallback) => void
  onStatus: (cb: StatusCallback) => void
  close: () => void
}

function wsUrl(notebookId: string): string {
  const httpBase = BASE_URL || window.location.origin
  const wsBase = httpBase.replace(/^http/, 'ws')
  return `${wsBase}/api/notebooks/${notebookId}/ws`
}

export function connectNotebook(notebookId: string): NotebookWebSocket {
  let ws: WebSocket | null = null
  let retryCount = 0
  let closed = false
  let retryTimer: ReturnType<typeof setTimeout> | null = null

  const messageCallbacks: MessageCallback[] = []
  const statusCallbacks: StatusCallback[] = []

  function emitStatus(status: 'connecting' | 'connected' | 'disconnected') {
    for (const cb of statusCallbacks) cb(status)
  }

  function connect() {
    if (closed) return

    emitStatus('connecting')
    ws = new WebSocket(wsUrl(notebookId))

    ws.onopen = () => {
      retryCount = 0
      emitStatus('connected')
    }

    ws.onmessage = (event: MessageEvent<string>) => {
      let msg: WsMessage
      try {
        msg = JSON.parse(event.data) as WsMessage
      } catch {
        return
      }
      for (const cb of messageCallbacks) cb(msg)
    }

    ws.onclose = () => {
      ws = null
      if (closed) {
        emitStatus('disconnected')
        return
      }
      if (retryCount >= MAX_RETRIES) {
        emitStatus('disconnected')
        return
      }
      const delay = BASE_BACKOFF_MS * Math.pow(2, retryCount)
      retryCount++
      emitStatus('disconnected')
      retryTimer = setTimeout(() => connect(), delay)
    }

    ws.onerror = () => {
      // onclose fires after onerror; handle reconnect there
    }
  }

  connect()

  return {
    sendExecute(cellId: string) {
      if (ws?.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'execute', cell_id: cellId }))
      }
    },

    onMessage(cb: MessageCallback) {
      messageCallbacks.push(cb)
    },

    onStatus(cb: StatusCallback) {
      statusCallbacks.push(cb)
    },

    close() {
      closed = true
      if (retryTimer !== null) {
        clearTimeout(retryTimer)
        retryTimer = null
      }
      ws?.close()
      ws = null
    },
  }
}
