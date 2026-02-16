import { useState } from 'react'
import { Play, ChevronDown, ChevronRight, AlertTriangle } from 'lucide-react'
import { useEvaluateRules } from '../api/hooks/useRulePlayground'
import { PageHeader } from '../components/layout/PageHeader'
import { Input } from '../components/ui/Input'
import { Button } from '../components/ui/Button'
import { Badge } from '../components/ui/Badge'
import { JsonViewer } from '../components/ui/JsonViewer'
import { useToast } from '../components/ui/useToast'
import type { EvaluateRulesResponse, RuleTraceEntry } from '../types'
import styles from './RulePlayground.module.css'

function resultBadgeClass(result: RuleTraceEntry['result']): string {
  switch (result) {
    case 'matched': return `${styles.resultBadge} ${styles.resultMatched}`
    case 'not_matched': return `${styles.resultBadge} ${styles.resultNotMatched}`
    case 'skipped': return `${styles.resultBadge} ${styles.resultSkipped}`
    case 'error': return `${styles.resultBadge} ${styles.resultError}`
  }
}

function verdictClass(verdict: string): string {
  const v = verdict.toLowerCase()
  if (v === 'allow') return styles.verdictAllow
  if (v === 'deny') return styles.verdictDeny
  if (v === 'suppress') return styles.verdictSuppress
  if (v === 'modify') return styles.verdictModify
  return styles.verdictError
}

function formatDuration(us: number): string {
  if (us < 1000) return `${us}us`
  if (us < 1_000_000) return `${(us / 1000).toFixed(1)}ms`
  return `${(us / 1_000_000).toFixed(2)}s`
}

function tryParseJson(text: string): { ok: true; value: Record<string, unknown> } | { ok: false; error: string } {
  try {
    const parsed = JSON.parse(text)
    return { ok: true, value: parsed }
  } catch (e) {
    return { ok: false, error: (e as Error).message }
  }
}

export function RulePlayground() {
  const evaluateMutation = useEvaluateRules()
  const { toast } = useToast()

  // Form state
  const [ns, setNs] = useState('default')
  const [tenant, setTenant] = useState('test-tenant')
  const [provider, setProvider] = useState('email')
  const [actionType, setActionType] = useState('notification')
  const [payload, setPayload] = useState('{\n  "to": "user@example.com",\n  "subject": "Hello"\n}')
  const [metadata, setMetadata] = useState('{}')
  const [mockState, setMockState] = useState('{}')
  const [includeDisabled, setIncludeDisabled] = useState(false)
  const [evaluateAll, setEvaluateAll] = useState(false)
  const [evaluateAt, setEvaluateAt] = useState('')

  // Validation
  const [payloadError, setPayloadError] = useState('')
  const [metadataError, setMetadataError] = useState('')
  const [mockStateError, setMockStateError] = useState('')

  // Result
  const [result, setResult] = useState<EvaluateRulesResponse | null>(null)

  // Trace expansion
  const [expandedRows, setExpandedRows] = useState<Set<number>>(new Set())
  const [contextOpen, setContextOpen] = useState(false)

  const toggleRow = (idx: number) => {
    setExpandedRows((prev) => {
      const next = new Set(prev)
      if (next.has(idx)) next.delete(idx)
      else next.add(idx)
      return next
    })
  }

  const handleEvaluate = () => {
    // Parse and validate JSON fields
    const parsedPayload = tryParseJson(payload)
    if (!parsedPayload.ok) {
      setPayloadError(`Invalid JSON: ${parsedPayload.error}`)
      return
    }
    setPayloadError('')

    const parsedMetadata = tryParseJson(metadata)
    if (!parsedMetadata.ok) {
      setMetadataError(`Invalid JSON: ${parsedMetadata.error}`)
      return
    }
    setMetadataError('')

    const parsedMockState = tryParseJson(mockState)
    if (!parsedMockState.ok) {
      setMockStateError(`Invalid JSON: ${parsedMockState.error}`)
      return
    }
    setMockStateError('')

    // Coerce all values to strings (backend expects HashMap<String, String>).
    const toStringRecord = (obj: Record<string, unknown>): Record<string, string> =>
      Object.fromEntries(Object.entries(obj).map(([k, v]) => [k, String(v)]))

    const metaObj = toStringRecord(parsedMetadata.value)
    const mockObj = toStringRecord(parsedMockState.value)

    // datetime-local produces "YYYY-MM-DDTHH:mm"; append ":00Z" for RFC 3339.
    const evaluateAtRfc = evaluateAt ? `${evaluateAt}:00Z` : null

    evaluateMutation.mutate(
      {
        namespace: ns,
        tenant,
        provider,
        action_type: actionType,
        payload: parsedPayload.value,
        metadata: Object.keys(metaObj).length > 0 ? metaObj : undefined,
        include_disabled: includeDisabled || undefined,
        evaluate_all: evaluateAll || undefined,
        evaluate_at: evaluateAtRfc,
        mock_state: Object.keys(mockObj).length > 0 ? mockObj : undefined,
      },
      {
        onSuccess: (res) => {
          setResult(res)
          setExpandedRows(new Set())
          setContextOpen(false)
        },
        onError: (e) => toast('error', 'Evaluation failed', (e as Error).message),
      },
    )
  }

  const isValid = ns && tenant && provider && actionType

  return (
    <div>
      <PageHeader
        title="Rule Playground"
        subtitle="Evaluate rules against a test action without dispatching"
      />

      <div className={styles.container}>
        <div className={styles.layout}>
          {/* Form panel */}
          <div className={styles.formCard}>
            <div className={styles.formGrid}>
              <Input
                label="Namespace *"
                value={ns}
                onChange={(e) => setNs(e.target.value)}
                placeholder="default"
              />
              <Input
                label="Tenant *"
                value={tenant}
                onChange={(e) => setTenant(e.target.value)}
                placeholder="test-tenant"
              />
              <Input
                label="Provider *"
                value={provider}
                onChange={(e) => setProvider(e.target.value)}
                placeholder="email"
              />
              <Input
                label="Action Type *"
                value={actionType}
                onChange={(e) => setActionType(e.target.value)}
                placeholder="notification"
              />
            </div>

            <div>
              <label className={styles.textareaLabel}>Payload (JSON) *</label>
              <textarea
                value={payload}
                onChange={(e) => {
                  setPayload(e.target.value)
                  if (payloadError) setPayloadError('')
                }}
                className={styles.textarea}
              />
              {payloadError && <p className={styles.warningText}>{payloadError}</p>}
            </div>

            <div>
              <label className={styles.textareaLabel}>Metadata (JSON)</label>
              <textarea
                value={metadata}
                onChange={(e) => {
                  setMetadata(e.target.value)
                  if (metadataError) setMetadataError('')
                }}
                className={styles.textareaSmall}
              />
              {metadataError && <p className={styles.warningText}>{metadataError}</p>}
            </div>

            <div className={styles.toggleRow}>
              <label className={styles.checkboxLabel}>
                <input
                  type="checkbox"
                  checked={includeDisabled}
                  onChange={(e) => setIncludeDisabled(e.target.checked)}
                  className={styles.checkbox}
                />
                Include Disabled Rules
              </label>
              <label className={styles.checkboxLabel}>
                <input
                  type="checkbox"
                  checked={evaluateAll}
                  onChange={(e) => setEvaluateAll(e.target.checked)}
                  className={styles.checkbox}
                />
                Evaluate All Rules
              </label>
            </div>

            <Input
              label="Evaluate At (time-travel)"
              type="datetime-local"
              value={evaluateAt}
              onChange={(e) => setEvaluateAt(e.target.value)}
            />

            <div>
              <label className={styles.textareaLabel}>Mock State (JSON)</label>
              <textarea
                value={mockState}
                onChange={(e) => {
                  setMockState(e.target.value)
                  if (mockStateError) setMockStateError('')
                }}
                className={styles.textareaSmall}
              />
              {mockStateError && <p className={styles.warningText}>{mockStateError}</p>}
            </div>

            <div className={styles.submitContainer}>
              <Button
                icon={<Play className="h-3.5 w-3.5" />}
                loading={evaluateMutation.isPending}
                onClick={handleEvaluate}
                disabled={!isValid}
              >
                Evaluate
              </Button>
            </div>
          </div>

          {/* Results panel */}
          <div>
            {!result && !evaluateMutation.isPending && (
              <div className={styles.resultsCard}>
                <p className={styles.emptyResults}>
                  Configure the action on the left and click Evaluate to see rule results.
                </p>
              </div>
            )}

            {result && (
              <div className={styles.resultsCard}>
                <h2 className={styles.resultsTitle}>Results</h2>

                {/* Summary bar */}
                <div className={`${styles.summaryBar} ${verdictClass(result.verdict)}`}>
                  <div>
                    <span className={styles.verdictLabel}>Verdict</span>
                    <div className={styles.verdictValue}>{result.verdict}</div>
                  </div>
                  {result.matched_rule && (
                    <div className={styles.summaryItem}>
                      Matched: <span className={styles.summaryItemValue}>{result.matched_rule}</span>
                    </div>
                  )}
                  <div className={styles.summaryItem}>
                    Evaluated: <span className={styles.summaryItemValue}>{result.total_rules_evaluated}</span>
                  </div>
                  <div className={styles.summaryItem}>
                    Skipped: <span className={styles.summaryItemValue}>{result.total_rules_skipped}</span>
                  </div>
                  <div className={styles.summaryItem}>
                    Time: <span className={styles.summaryItemValue}>{formatDuration(result.evaluation_duration_us)}</span>
                  </div>
                </div>

                {/* Error banner */}
                {result.has_errors && (
                  <div className={styles.errorBanner}>
                    <AlertTriangle className="h-4 w-4 flex-shrink-0" />
                    Some rules encountered errors during evaluation. Check the trace below for details.
                  </div>
                )}

                {/* Trace table */}
                {result.trace.length > 0 && (
                  <div className={styles.traceSection}>
                    <h3 className={styles.traceSectionTitle}>Rule Trace</h3>
                    <table className={styles.traceTable}>
                      <thead>
                        <tr>
                          <th></th>
                          <th>Rule Name</th>
                          <th>Pri</th>
                          <th>Result</th>
                          <th>Action</th>
                          <th>Duration</th>
                          <th>Source</th>
                        </tr>
                      </thead>
                      <tbody>
                        {result.trace.map((entry, idx) => (
                          <TraceRow
                            key={idx}
                            entry={entry}
                            expanded={expandedRows.has(idx)}
                            onToggle={() => toggleRow(idx)}
                          />
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}

                {/* Context section */}
                <div className={styles.contextSection}>
                  <div
                    className={styles.contextHeader}
                    onClick={() => setContextOpen(!contextOpen)}
                  >
                    <span>Context</span>
                    {contextOpen
                      ? <ChevronDown className="h-4 w-4" />
                      : <ChevronRight className="h-4 w-4" />
                    }
                  </div>
                  {contextOpen && (
                    <div className={styles.contextBody}>
                      <span className={styles.contextLabel}>Time</span>
                      <div className={styles.payloadPre}>
                        {JSON.stringify(result.context.time, null, 2)}
                      </div>
                      {result.context.effective_timezone && (
                        <div style={{ marginTop: '0.5rem' }}>
                          <span className={styles.contextLabel}>Timezone</span>
                          <p>{result.context.effective_timezone}</p>
                        </div>
                      )}
                      {result.context.environment_keys.length > 0 && (
                        <div style={{ marginTop: '0.5rem' }}>
                          <span className={styles.contextLabel}>Environment Keys</span>
                          <p>{result.context.environment_keys.join(', ')}</p>
                        </div>
                      )}
                      {result.context.accessed_state_keys && result.context.accessed_state_keys.length > 0 && (
                        <div style={{ marginTop: '0.5rem' }}>
                          <span className={styles.contextLabel}>Accessed State Keys</span>
                          <p>{result.context.accessed_state_keys.join(', ')}</p>
                        </div>
                      )}
                    </div>
                  )}
                </div>

                {/* Modified Payload */}
                {result.verdict.toLowerCase() === 'modify' && result.modified_payload && (
                  <div>
                    <h3 className={styles.traceSectionTitle}>Modified Payload</h3>
                    <div className={styles.payloadPre}>
                      <JsonViewer data={result.modified_payload} />
                    </div>
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}

// ---- Trace Row ----

function TraceRow({ entry, expanded, onToggle }: {
  entry: RuleTraceEntry
  expanded: boolean
  onToggle: () => void
}) {
  return (
    <>
      <tr onClick={onToggle} style={{ cursor: 'pointer' }}>
        <td>
          {expanded
            ? <ChevronDown className="h-3.5 w-3.5" />
            : <ChevronRight className="h-3.5 w-3.5" />
          }
        </td>
        <td className={styles.ruleNameCell}>{entry.rule_name}</td>
        <td className={styles.priorityCell}>{entry.priority}</td>
        <td>
          <span className={resultBadgeClass(entry.result)}>
            {entry.result.replace('_', ' ')}
          </span>
        </td>
        <td><Badge>{entry.action}</Badge></td>
        <td className={styles.durationCell}>{formatDuration(entry.evaluation_duration_us)}</td>
        <td className={styles.sourceCell}>{entry.source}</td>
      </tr>
      {expanded && (
        <tr className={styles.traceDetailRow}>
          <td colSpan={7}>
            <div className={styles.traceDetailContent}>
              <div>
                <span className={styles.traceDetailLabel}>Condition: </span>
                {entry.condition_display}
              </div>
              <div>
                <span className={styles.traceDetailLabel}>Enabled: </span>
                {entry.enabled ? 'Yes' : 'No'}
              </div>
              {entry.description && (
                <div>
                  <span className={styles.traceDetailLabel}>Description: </span>
                  {entry.description}
                </div>
              )}
              {entry.skip_reason && (
                <div>
                  <span className={styles.traceDetailLabel}>Skip Reason: </span>
                  {entry.skip_reason}
                </div>
              )}
              {entry.error && (
                <div>
                  <span className={styles.traceDetailLabel}>Error: </span>
                  <span className={styles.resultError}>{entry.error}</span>
                </div>
              )}
              {entry.semantic_details && (
                <div className={styles.semanticSection}>
                  <div className={styles.semanticTitle}>Semantic Match</div>
                  <div className={styles.semanticGrid}>
                    <span className={styles.traceDetailLabel}>Topic</span>
                    <span className={styles.semanticValue}>{entry.semantic_details.topic}</span>
                    <span className={styles.traceDetailLabel}>Extracted Text</span>
                    <span className={styles.semanticValue}>{entry.semantic_details.extracted_text}</span>
                    <span className={styles.traceDetailLabel}>Similarity</span>
                    <span className={styles.semanticValue}>{(entry.semantic_details.similarity * 100).toFixed(1)}%</span>
                    <span className={styles.traceDetailLabel}>Threshold</span>
                    <span className={styles.semanticValue}>{(entry.semantic_details.threshold * 100).toFixed(1)}%</span>
                    <div className={styles.similarityBar}>
                      <div className={styles.similarityTrack}>
                        <div
                          className={`${styles.similarityFill} ${
                            entry.semantic_details.similarity >= entry.semantic_details.threshold
                              ? styles.similarityPass
                              : styles.similarityFail
                          }`}
                          style={{ width: `${Math.min(entry.semantic_details.similarity * 100, 100)}%` }}
                        />
                      </div>
                      <span className={styles.semanticValue}>
                        {entry.semantic_details.similarity >= entry.semantic_details.threshold ? 'pass' : 'fail'}
                      </span>
                    </div>
                  </div>
                </div>
              )}
              {entry.wasm_details && (
                <div className={styles.semanticSection}>
                  <div className={styles.semanticTitle}>WASM Plugin</div>
                  <div className={styles.semanticGrid}>
                    <span className={styles.traceDetailLabel}>Plugin</span>
                    <span className={styles.semanticValue}>{entry.wasm_details.plugin}</span>
                    <span className={styles.traceDetailLabel}>Function</span>
                    <span className={styles.semanticValue}>{entry.wasm_details.function}</span>
                    <span className={styles.traceDetailLabel}>Verdict</span>
                    <span className={styles.semanticValue}>{entry.wasm_details.verdict ? 'true (pass)' : 'false (fail)'}</span>
                    <span className={styles.traceDetailLabel}>Duration</span>
                    <span className={styles.semanticValue}>{entry.wasm_details.duration_us}us</span>
                    {entry.wasm_details.message && (
                      <>
                        <span className={styles.traceDetailLabel}>Message</span>
                        <span className={styles.semanticValue}>{entry.wasm_details.message}</span>
                      </>
                    )}
                    {entry.wasm_details.memory_used_bytes != null && (
                      <>
                        <span className={styles.traceDetailLabel}>Memory Used</span>
                        <span className={styles.semanticValue}>
                          {entry.wasm_details.memory_used_bytes >= 1_048_576
                            ? `${(entry.wasm_details.memory_used_bytes / 1_048_576).toFixed(1)} MB`
                            : `${(entry.wasm_details.memory_used_bytes / 1024).toFixed(0)} KB`}
                        </span>
                      </>
                    )}
                  </div>
                </div>
              )}
              {entry.modify_patch && (
                <div className={styles.traceDetailJsonSection}>
                  <span className={styles.traceDetailJsonLabel}>Modify Patch</span>
                  <div className={styles.traceDetailJsonPre}>
                    <JsonViewer data={entry.modify_patch} />
                  </div>
                </div>
              )}
              {entry.modified_payload_preview && (
                <div className={styles.traceDetailJsonSection}>
                  <span className={styles.traceDetailJsonLabel}>Modified Payload Preview</span>
                  <div className={styles.traceDetailJsonPre}>
                    <JsonViewer data={entry.modified_payload_preview} />
                  </div>
                </div>
              )}
            </div>
          </td>
        </tr>
      )}
    </>
  )
}
