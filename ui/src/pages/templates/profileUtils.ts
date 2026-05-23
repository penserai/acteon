import type { TemplateProfileField } from '../../types'

export interface FieldEntry {
  key: string
  valueType: 'inline' | 'ref'
  inlineValue: string
  refValue: string
}

export function emptyEntry(): FieldEntry {
  return { key: '', valueType: 'inline', inlineValue: '', refValue: '' }
}

export function fieldsToEntries(
  fields: Record<string, TemplateProfileField>,
): FieldEntry[] {
  return Object.entries(fields).map(([key, val]) => {
    if (typeof val === 'string') {
      return { key, valueType: 'inline', inlineValue: val, refValue: '' }
    }
    return { key, valueType: 'ref', inlineValue: '', refValue: val.$ref }
  })
}

export function entriesToFields(
  entries: FieldEntry[],
): Record<string, TemplateProfileField> {
  const result: Record<string, TemplateProfileField> = {}
  for (const entry of entries) {
    if (!entry.key) continue
    if (entry.valueType === 'ref') {
      result[entry.key] = { $ref: entry.refValue }
    } else {
      result[entry.key] = entry.inlineValue
    }
  }
  return result
}
