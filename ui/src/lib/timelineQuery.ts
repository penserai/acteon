import type { TimelineQuery } from '../api/timeline'

export type AttributeType = 'string' | 'number' | 'boolean' | 'null' | 'exists'
export interface AttributeCondition { id: number; path: string; type: AttributeType; value: string }
export interface TimelineQueryDraft {
  group: string
  eventType: string
  filter: string
  text: string
  textPath: string
  textMode: 'all' | 'any'
  from: string
  to: string
  order: 'auto' | 'oldest' | 'newest'
  limit: string
  attributes: AttributeCondition[]
}
export function emptyTimelineDraft(): TimelineQueryDraft {
  return {
    group: '', eventType: '', filter: '', text: '', textPath: '', textMode: 'all',
    from: '', to: '', order: 'auto', limit: '50',
    attributes: [{ id: 0, path: '', type: 'string', value: '' }],
  }
}
function validatePath(path: string) {
  if (!path.startsWith('/') || /~(?![01])/.test(path) || new TextEncoder().encode(path).length > 4096) {
    throw new Error('Attribute paths must be JSON Pointers, such as /cluster/id. Escape ~ as ~0 and / as ~1 within a key.')
  }
}
function dateBound(value: string, label: string): string | undefined {
  if (!value) return undefined
  const date = new Date(value)
  if (!Number.isFinite(date.getTime()) || date.getTime() < 0 || date.getTime() > 9223372036854) {
    throw new Error(`${label} must be a valid date within 1970–2262.`)
  }
  return date.toISOString()
}
export function buildTimelineQuery(draft: TimelineQueryDraft): TimelineQuery {
  const from = dateBound(draft.from, 'From')
  const to = dateBound(draft.to, 'To')
  if (from && to && Date.parse(from) > Date.parse(to)) throw new Error('From must be before or equal to To.')
  if (new TextEncoder().encode(draft.filter).length > 4096) throw new Error('CEL filters are limited to 4,096 bytes.')
  const equals: NonNullable<TimelineQuery['equals']> = []
  const exists: string[] = []
  for (const row of draft.attributes) {
    if (!row.path && !row.value && row.type === 'string') continue
    validatePath(row.path)
    if (row.type === 'exists') { exists.push(row.path); continue }
    let value: string | number | boolean | null = row.value
    if (row.type === 'null') value = null
    else if (row.type === 'boolean') value = row.value === 'true'
    else if (row.type === 'number') {
      try { value = JSON.parse(row.value) as number } catch { throw new Error(`Enter a valid JSON number for ${row.path}.`) }
      if (typeof value !== 'number' || !Number.isFinite(value)) throw new Error(`Enter a valid JSON number for ${row.path}.`)
      if (Number.isInteger(value) && (/[.eE]/.test(row.value) || Object.is(value, -0))) throw new Error('Use CEL to match a decimal such as 2.0 or negative zero without changing its numeric representation.')
      if (Number.isInteger(value) && !Number.isSafeInteger(value)) throw new Error('This integer is too large for the attribute form. Use an exact integer literal in the CEL filter.')
    }
    equals.push({ path: row.path, value })
  }
  if (draft.textPath) {
    validatePath(draft.textPath)
    if (!draft.text.trim()) throw new Error('Enter search words to use a text attribute path.')
  }
  return {
    eventGroup: draft.group || undefined,
    eventType: draft.eventType || undefined,
    filter: draft.filter.trim() || undefined,
    text: draft.text.trim() ? { text: draft.text, path: draft.textPath || undefined, mode: draft.textMode } : undefined,
    equals, exists, from, to,
    ascending: draft.order === 'auto' ? undefined : draft.order === 'oldest',
    limit: Number(draft.limit),
  }
}
