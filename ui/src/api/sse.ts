import { apiResponse } from './client'

export interface ServerEvent { type: string; data: string; id?: string }

/** Streaming fetch carries the same credential headers as every other API call. */
export function openEventStream(path: string, handlers: {
  open: () => void
  event: (event: ServerEvent) => void
  error: (error: unknown) => void
}, lastEventId?: string): AbortController {
  const controller = new AbortController()
  void (async () => {
    try {
      const response = await apiResponse(path, {
        signal: controller.signal,
        headers: { Accept: 'text/event-stream', ...(lastEventId ? { 'Last-Event-ID': lastEventId } : {}) },
      })
      if (!response.body || !response.headers.get('content-type')?.startsWith('text/event-stream')) {
        throw new Error('Expected an event stream')
      }
      const reader = response.body.pipeThrough(new TextDecoderStream()).getReader()
      handlers.open()
      let buffer = ''
      let type = 'message'
      let data: string[] = []
      let id: string | undefined
      try {
        while (!controller.signal.aborted) {
          const { value, done } = await reader.read()
          if (done) break
          buffer += value
          // SSE supports LF, CRLF and CR. Retain a trailing CR until the next chunk.
          let match: RegExpExecArray | null
          while ((match = /\r\n|\r(?!$)|\n/.exec(buffer))) {
            const line = buffer.slice(0, match.index)
            buffer = buffer.slice(match.index + match[0].length)
            if (!line) {
              if (data.length) handlers.event({ type, data: data.join('\n'), id })
              type = 'message'; data = []; id = undefined
            } else if (!line.startsWith(':')) {
              const colon = line.indexOf(':')
              const field = colon < 0 ? line : line.slice(0, colon)
              const value = colon < 0 ? '' : line.slice(colon + 1).replace(/^ /, '')
              if (field === 'event') type = value
              else if (field === 'data') data.push(value)
              else if (field === 'id' && !value.includes('\0')) id = value
            }
            if (controller.signal.aborted) break
          }
        }
      } finally {
        await reader.cancel().catch(() => {})
        reader.releaseLock()
      }
      if (!controller.signal.aborted) handlers.error(new Error('Event stream closed'))
    } catch (error) {
      if (!controller.signal.aborted) handlers.error(error)
    }
  })()
  return controller
}
