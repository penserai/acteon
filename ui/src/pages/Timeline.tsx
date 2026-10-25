import { useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { ChevronLeft, ChevronRight, Plus, Search, X } from 'lucide-react'
import {
  getTimelineLedgerStatus, queryTimeline, timelineErrorMessage,
  type TimelineEvent, type TimelineLedgerStatus, type TimelinePage, type TimelineQuery,
} from '../api/timeline'
import { buildTimelineQuery, emptyTimelineDraft, type AttributeCondition, type TimelineQueryDraft } from '../lib/timelineQuery'
import { PageHeader } from '../components/layout/PageHeader'
import { Button } from '../components/ui/Button'
import { EmptyState } from '../components/ui/EmptyState'
import { Input } from '../components/ui/Input'
import { JsonViewer } from '../components/ui/JsonViewer'
import { Select } from '../components/ui/Select'
import styles from './Timeline.module.css'

const examples = [
  { label: 'Exact match', expression: 'attributes.cluster.id == "cluster-42"' },
  { label: 'Alternatives', expression: 'attributes.region in ["us-west-2", "eu-west-1"]' },
  { label: 'Missing attribute', expression: '!has(attributes.error)' },
  { label: 'Text search', expression: 'textMatch("deployment completed")' },
]
const attributeTypes = [
  { value: 'string', label: 'String' }, { value: 'number', label: 'Number' },
  { value: 'boolean', label: 'Boolean' }, { value: 'null', label: 'Null' }, { value: 'exists', label: 'Exists' },
]
interface SubmittedQuery { query: TimelineQuery; draft: string }
function displayDate(value: string) { return new Date(value).toLocaleString() }

export function Timeline() {
  const [draft, setDraft] = useState(emptyTimelineDraft)
  const [submitted, setSubmitted] = useState<SubmittedQuery | null>(null)
  const [pages, setPages] = useState<TimelinePage[]>([])
  const [pageIndex, setPageIndex] = useState(0)
  const [status, setStatus] = useState<TimelineLedgerStatus | null>(null)
  const [error, setError] = useState('')
  const [ledgerError, setLedgerError] = useState('')
  const [busy, setBusy] = useState(false)
  const [ledgerBusy, setLedgerBusy] = useState(false)
  const [selected, setSelected] = useState<TimelineEvent | null>(null)
  const request = useRef(0)
  const queryController = useRef<AbortController | null>(null)
  const ledgerController = useRef<AbortController | null>(null)
  const rowId = useRef(1)
  const celInput = useRef<HTMLTextAreaElement>(null)
  const detailPanel = useRef<HTMLElement>(null)
  const selectedButton = useRef<HTMLButtonElement | null>(null)
  const page = pages[pageIndex]
  const dirty = !!submitted && submitted.draft !== JSON.stringify(draft)

  useEffect(() => () => { queryController.current?.abort(); ledgerController.current?.abort() }, [])
  useEffect(() => {
    if (selected) { detailPanel.current?.focus(); detailPanel.current?.scrollIntoView({ block: 'nearest' }) }
  }, [selected])

  function update<K extends keyof TimelineQueryDraft>(key: K, value: TimelineQueryDraft[K]) {
    setDraft(current => ({ ...current, [key]: value }))
  }
  function updateAttribute(id: number, patch: Partial<AttributeCondition>) {
    setDraft(current => ({ ...current, attributes: current.attributes.map(row => row.id === id ? { ...row, ...patch } : row) }))
  }
  async function fetchPage(search: SubmittedQuery, next = false) {
    const sequence = ++request.current
    queryController.current?.abort()
    const controller = new AbortController()
    queryController.current = controller
    setBusy(true); setError(''); setSelected(null)
    try {
      const result = await queryTimeline({ ...search.query, cursor: next ? page?.nextCursor ?? undefined : undefined }, controller.signal)
      if (sequence !== request.current || controller.signal.aborted) return
      setPages(next ? [...pages, result] : [result])
      setPageIndex(next ? pageIndex + 1 : 0)
      setSubmitted(search)
    } catch (e) {
      if (sequence === request.current && !controller.signal.aborted) setError(timelineErrorMessage(e))
    } finally {
      if (sequence === request.current && !controller.signal.aborted) setBusy(false)
    }
  }
  function search() {
    try {
      const query = buildTimelineQuery(draft)
      void fetchPage({ query, draft: JSON.stringify(draft) })
    } catch (e) { setError(timelineErrorMessage(e)) }
  }
  function nextPage() {
    if (pages[pageIndex + 1]) { setPageIndex(pageIndex + 1); setSelected(null); setError('') }
    else if (submitted && page?.nextCursor) void fetchPage(submitted, true)
  }
  async function inspectLedger() {
    ledgerController.current?.abort()
    const controller = new AbortController()
    ledgerController.current = controller
    setLedgerBusy(true); setLedgerError('')
    try {
      const result = await getTimelineLedgerStatus(controller.signal)
      if (!controller.signal.aborted) setStatus(result)
    } catch (e) { if (!controller.signal.aborted) { setStatus(null); setLedgerError(timelineErrorMessage(e)) } }
    finally { if (!controller.signal.aborted) setLedgerBusy(false) }
  }
  function resetFilters() { setDraft(emptyTimelineDraft()); setError('') }
  function addExample(expression: string) {
    update('filter', draft.filter.trim() ? `(${draft.filter.trim()}) && (${expression})` : expression)
    celInput.current?.focus()
  }

  return <>
    <PageHeader title="Event Timeline" subtitle="Follow a deployment or find events across a tenant’s units of work." actions={<Link to="/retention">Retention policies</Link>} />
    <form className={styles.queryPanel} onSubmit={e => { e.preventDefault(); search() }}>
      <div className={styles.scopeGrid}>
        <Input label="Event group" value={draft.group} onChange={e => update('group', e.target.value)} placeholder="cluster-deployment-42" />
        <Input label="Event type" value={draft.eventType} onChange={e => update('eventType', e.target.value)} placeholder="deployment.ready" />
      </div>
      <div className={styles.cel}>
        <label htmlFor="timeline-cel">CEL filter <span>Optional</span></label>
        <textarea id="timeline-cel" ref={celInput} value={draft.filter} rows={3} spellCheck={false} maxLength={4096}
          onChange={e => update('filter', e.target.value)}
          onKeyDown={e => { if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') { e.preventDefault(); e.currentTarget.form?.requestSubmit() } }}
          placeholder={'attributes.environment == "production" && !has(attributes.error)'} aria-describedby="timeline-cel-help" />
        <p id="timeline-cel-help" className={styles.hint}>Combine conditions with &amp;&amp;, || and !. Use == for exact values, including UUIDs. Ctrl/⌘ + Enter searches.</p>
        <details className={styles.examples}>
          <summary>CEL examples and syntax</summary>
          <div className={styles.exampleList}>{examples.map(example => <div key={example.label}>
            <code>{example.expression}</code><Button type="button" size="sm" variant="ghost" onClick={() => addExample(example.expression)}>Add {example.label.toLowerCase()} example</Button>
          </div>)}</div>
          <p className={styles.hint}>Use has(attributes.key) for presence, or "blue" in attributes.tags for an array member. Numeric attribute ranges are not supported. All additional filters below combine with CEL using AND.</p>
        </details>
      </div>
      <details className={styles.additional}>
        <summary>Additional filters</summary>
        <div className={styles.extraFilters}>
          <div className={styles.textGrid}>
            <Input label="Full-text search" value={draft.text} onChange={e => update('text', e.target.value)} placeholder="deployment completed" />
            <Input label="Text attribute path" value={draft.textPath} onChange={e => update('textPath', e.target.value)} placeholder="All attributes and message" />
            <Select label="Match words" value={draft.textMode} onChange={e => update('textMode', e.target.value as 'all' | 'any')} options={[{ value: 'all', label: 'All words' }, { value: 'any', label: 'Any word' }]} />
          </div>
          <div className={styles.timeGrid}>
            <Input label="From" type="datetime-local" step="1" value={draft.from} onChange={e => update('from', e.target.value)} />
            <Input label="To" type="datetime-local" step="1" value={draft.to} onChange={e => update('to', e.target.value)} />
          </div>
          <p className={styles.hint}>Occurrence times are inclusive. Enter dates in your local timezone ({Intl.DateTimeFormat().resolvedOptions().timeZone}); requests use UTC.</p>
          <div>
            <h2 className={styles.sectionTitle}>Attribute conditions</h2>
            <p className={styles.hint}>All conditions must match. Paths use JSON Pointer syntax, such as /cluster/id. Strings match exactly, including case. Scalar array members also match; use CEL for strict scalar equality.</p>
            {draft.attributes.map((row, index) => <fieldset key={row.id} className={styles.attribute}>
              <legend>Attribute {index + 1}</legend>
              <div className={styles.attributeGrid}>
                <Input id={`attribute-path-${row.id}`} label="Attribute path" value={row.path} onChange={e => updateAttribute(row.id, { path: e.target.value })} placeholder="/customId" />
                <Select id={`attribute-type-${row.id}`} label="Value type" value={row.type} options={attributeTypes} onChange={e => updateAttribute(row.id, { type: e.target.value as AttributeCondition['type'], value: e.target.value === 'boolean' ? 'true' : '' })} />
                {row.type === 'boolean' ? <Select id={`attribute-value-${row.id}`} label="Exact boolean value" value={row.value} onChange={e => updateAttribute(row.id, { value: e.target.value })} options={[{ value: 'true', label: 'True' }, { value: 'false', label: 'False' }]} />
                  : row.type === 'null' || row.type === 'exists' ? <p className={styles.hint}>{row.type === 'null' ? 'Value must be null.' : 'Any present value, including null.'}</p>
                    : <Input id={`attribute-value-${row.id}`} label={`Exact ${row.type} value`} value={row.value} onChange={e => updateAttribute(row.id, { value: e.target.value })} placeholder={row.type === 'number' ? '42' : 'UUID or other literal value'} />}
                <Button type="button" variant="ghost" aria-label={`Remove attribute ${index + 1}`} onClick={() => update('attributes', draft.attributes.filter(item => item.id !== row.id))}><X size={16} /></Button>
              </div>
            </fieldset>)}
            <Button type="button" variant="secondary" size="sm" icon={<Plus size={14} />} disabled={draft.attributes.length >= 64} onClick={() => update('attributes', [...draft.attributes, { id: rowId.current++, path: '', type: 'string', value: '' }])}>Add attribute</Button>
          </div>
        </div>
      </details>
      <div className={styles.toolbar}>
        <Select label="Order" value={draft.order} onChange={e => update('order', e.target.value as TimelineQueryDraft['order'])} options={[{ value: 'auto', label: 'Automatic' }, { value: 'oldest', label: 'Oldest first' }, { value: 'newest', label: 'Newest first' }]} />
        <Select label="Page size" value={draft.limit} onChange={e => update('limit', e.target.value)} options={['25', '50', '100', '250', '1000'].map(value => ({ value, label: `${value} events` }))} />
        <div className={styles.actions}>
          <Button type="submit" disabled={busy} icon={<Search size={16} />}>{busy ? 'Searching…' : 'Search events'}</Button>
          <Button type="button" variant="ghost" onClick={resetFilters}>Reset filters</Button>
        </div>
      </div>
      <p className={styles.hint}>Automatic order is oldest first when the Event group field is set, newest first otherwise.</p>
    </form>
    {error && <div role="alert" className={styles.error}><strong>Query failed</strong><p>{error}</p>{page && <p>Last successful results remain below.</p>}</div>}
    {dirty && <p role="status" className={styles.notice}>Filters have changed. Search events to apply them; pagination uses the last submitted query.</p>}
    {!page && !busy && !error && <EmptyState icon={<Search size={28} />} title="Find events for a unit of work" description="Search your timeline. Add an event group for a deployment timeline or use CEL to combine attributes." />}
    {busy && !page && <p role="status" className={styles.hint}>Searching timeline events…</p>}
    {page && submitted && <section aria-label="Timeline results" aria-busy={busy} className={styles.results}>
      <div className={styles.resultsHeader}>
        <h2>Results</h2><p role="status">{page.events.length} {page.events.length === 1 ? 'event' : 'events'} on page {pageIndex + 1}</p>
      </div>
      {page.events.length === 0 && <EmptyState title="No matching events" description="Try a wider time range or remove a condition. Only current, unexpired events are searchable." />}
      <ol className={styles.events}>{page.events.map(event => <li key={JSON.stringify([event.source, event.eventId])}>
        <button className={styles.event} onClick={e => { selectedButton.current = e.currentTarget; setSelected(event) }}>
          <time dateTime={event.event.occurredAt}>{displayDate(event.event.occurredAt)}</time>
          <div><strong>{event.event.eventType}</strong><p>{event.event.message ?? event.eventId}</p>
            <dl><div><dt>Group</dt><dd>{event.event.eventGroup}</dd></div><div><dt>Source</dt><dd>{event.source}</dd></div><div><dt>Revision</dt><dd>{event.revision}</dd></div></dl>
          </div><ChevronRight size={16} aria-hidden="true" />
        </button>
      </li>)}</ol>
      <nav className={styles.pagination} aria-label="Timeline pagination">
        <Button variant="secondary" disabled={busy || pageIndex === 0} icon={<ChevronLeft size={16} />} onClick={() => { setPageIndex(pageIndex - 1); setSelected(null); setError('') }}>Previous page</Button>
        <span>Page {pageIndex + 1}</span>
        <Button variant="secondary" disabled={busy || (!page.nextCursor && !pages[pageIndex + 1])} onClick={nextPage}>Next page<ChevronRight size={16} /></Button>
      </nav>
      <p className={styles.hint}>New pages read live data. Previous pages show previously fetched results; search again to refresh.</p>
    </section>}
    {selected && <section ref={detailPanel} tabIndex={-1} aria-label="Event details" className={styles.details}>
      <div className={styles.resultsHeader}><h2>{selected.event.eventType}</h2><Button variant="ghost" onClick={() => { setSelected(null); selectedButton.current?.focus() }} aria-label="Close event details"><X size={16} /></Button></div>
      <dl className={styles.metadata}>{[
        ['Event ID', selected.eventId], ['Source', selected.source], ['Event group', selected.event.eventGroup],
        ['Revision', selected.revision], ['Occurred', displayDate(selected.event.occurredAt)],
        ['First ingested', displayDate(selected.firstIngestedAt)], ['Updated', selected.updatedAt ? displayDate(selected.updatedAt) : '—'], ['Expires', displayDate(selected.expiresAt)],
      ].map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>
      <h3 className={styles.sectionTitle}>Event data</h3><div className={styles.payload}><JsonViewer data={selected.event} /></div>
    </section>}
    <div className={styles.ledger}>
      <Button type="button" variant="ghost" size="sm" disabled={ledgerBusy} onClick={() => void inspectLedger()}>{ledgerBusy ? 'Loading ledger…' : 'Ledger status'}</Button>
      {ledgerError && <p role="alert" className={styles.error}>{ledgerError}</p>}
      {status && <p role="status">{status.records.toLocaleString()} retained CDC records; {(status.bytes / 1024 / 1024).toFixed(1)} MiB; {status.pending} pending.</p>}
    </div>
  </>
}
