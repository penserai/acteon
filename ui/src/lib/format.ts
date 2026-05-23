import { formatDistanceToNowStrict, format } from 'date-fns'

export function relativeTime(date: string | Date): string {
  const d = typeof date === 'string' ? new Date(date) : date
  return formatDistanceToNowStrict(d, { addSuffix: true })
}

export function absoluteTime(date: string | Date): string {
  const d = typeof date === 'string' ? new Date(date) : date
  return format(d, 'yyyy-MM-dd HH:mm:ss')
}

export function shortTime(date: string | Date): string {
  const d = typeof date === 'string' ? new Date(date) : date
  return format(d, 'HH:mm:ss')
}

export function formatNumber(n: number): string {
  return new Intl.NumberFormat().format(n)
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`
  return `${Math.floor(ms / 60000)}m ${Math.floor((ms % 60000) / 1000)}s`
}

export function formatCountdown(targetDate: string | Date): string {
  const target = typeof targetDate === 'string' ? new Date(targetDate) : targetDate
  const now = new Date()
  const diff = target.getTime() - now.getTime()
  if (diff <= 0) return 'Expired'
  const minutes = Math.floor(diff / 60000)
  const seconds = Math.floor((diff % 60000) / 1000)
  if (minutes > 60) return `${Math.floor(minutes / 60)}h ${minutes % 60}m`
  return `${minutes}m ${seconds}s`
}

export function tryParseJson(text: string): { ok: true; value: Record<string, unknown> } | { ok: false; error: string } {
  try {
    const parsed = JSON.parse(text)
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
      return { ok: false, error: 'Payload must be a JSON object' }
    }
    return { ok: true, value: parsed }
  } catch (e) {
    return { ok: false, error: (e as Error).message }
  }
}

export function parseLabels(text: string): Record<string, string> {
  const labels: Record<string, string> = {}
  for (const line of text.split('\n')) {
    const trimmed = line.trim()
    if (!trimmed) continue
    const eqIdx = trimmed.indexOf('=')
    if (eqIdx > 0) {
      labels[trimmed.slice(0, eqIdx).trim()] = trimmed.slice(eqIdx + 1).trim()
    }
  }
  return labels
}

export function labelsToText(labels: Record<string, string>): string {
  return Object.entries(labels).map(([k, v]) => `${k}=${v}`).join('\n')
}

export function formatBytes(bytes: number): string {
  if (bytes >= 1_048_576) return `${(bytes / 1_048_576).toFixed(0)} MB`
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`
  return `${bytes} B`
}

export function formatDurationSeconds(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return 'Not set'
  if (seconds === 0) return 'Immediate'
  const days = Math.floor(seconds / 86400)
  const hours = Math.floor((seconds % 86400) / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  const parts = []
  if (days > 0) parts.push(`${days}d`)
  if (hours > 0) parts.push(`${hours}h`)
  if (minutes > 0) parts.push(`${minutes}m`)
  return parts.length > 0 ? parts.join(' ') : `${seconds}s`
}

export function formatDurationMicros(us: number): string {
  if (us < 1000) return `${us}us`
  if (us < 1_000_000) return `${(us / 1000).toFixed(1)}ms`
  return `${(us / 1_000_000).toFixed(2)}s`
}

export function formatCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return n.toString()
}

export function capitalize(word: string): string {
  return word.charAt(0).toUpperCase() + word.slice(1)
}
