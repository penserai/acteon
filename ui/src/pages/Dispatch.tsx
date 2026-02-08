import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Send } from 'lucide-react'
import { useDispatch } from '../api/hooks/useActions'
import { PageHeader } from '../components/layout/PageHeader'
import { Input } from '../components/ui/Input'
import { Button } from '../components/ui/Button'
import { Badge } from '../components/ui/Badge'
import { JsonViewer } from '../components/ui/JsonViewer'
import { useToast } from '../components/ui/Toast'
import type { DispatchRequest, DispatchResponse } from '../types'
import styles from './Dispatch.module.css'

export function Dispatch() {
  const dispatch = useDispatch()
  const { toast } = useToast()
  const navigate = useNavigate()
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')
  const [provider, setProvider] = useState('')
  const [actionType, setActionType] = useState('')
  const [payload, setPayload] = useState('{\n  \n}')
  const [dedupKey, setDedupKey] = useState('')
  const [dryRun, setDryRun] = useState(false)
  const [result, setResult] = useState<DispatchResponse | null>(null)
  const [payloadError, setPayloadError] = useState('')

  const handleDispatch = () => {
    let parsed: Record<string, unknown>
    try {
      parsed = JSON.parse(payload)
      setPayloadError('')
    } catch {
      setPayloadError('Invalid JSON')
      return
    }

    const request: DispatchRequest = {
      namespace: ns,
      tenant,
      provider,
      action_type: actionType,
      payload: parsed,
      dedup_key: dedupKey || undefined,
    }

    dispatch.mutate({ request, dryRun }, {
      onSuccess: (res) => {
        setResult(res)
        toast('success', 'Action dispatched', `ID: ${res.action_id}`)
      },
      onError: (e) => toast('error', 'Dispatch failed', (e as Error).message),
    })
  }

  return (
    <div>
      <PageHeader title="Dispatch Action" />

      <div className={styles.container}>
        <div className={styles.formCard}>
          <div className={styles.formGrid}>
            <Input label="Namespace *" value={ns} onChange={(e) => setNs(e.target.value)} placeholder="prod" />
            <Input label="Tenant *" value={tenant} onChange={(e) => setTenant(e.target.value)} placeholder="acme" />
            <Input label="Provider *" value={provider} onChange={(e) => setProvider(e.target.value)} placeholder="email" />
            <Input label="Action Type *" value={actionType} onChange={(e) => setActionType(e.target.value)} placeholder="send-notification" />
          </div>

          <div>
            <label className={styles.textareaLabel}>Payload (JSON) *</label>
            <textarea
              value={payload}
              onChange={(e) => setPayload(e.target.value)}
              className={styles.textarea}
            />
            {payloadError && <p className={styles.errorText}>{payloadError}</p>}
          </div>

          <Input label="Dedup Key" value={dedupKey} onChange={(e) => setDedupKey(e.target.value)} placeholder="Optional" />

          <label className={styles.checkboxLabel}>
            <input
              type="checkbox"
              checked={dryRun}
              onChange={(e) => setDryRun(e.target.checked)}
              className={styles.checkbox}
            />
            Dry Run
          </label>

          <div className={styles.submitContainer}>
            <Button
              icon={<Send className="h-3.5 w-3.5" />}
              loading={dispatch.isPending}
              onClick={handleDispatch}
              disabled={!ns || !tenant || !provider || !actionType}
            >
              Dispatch Action
            </Button>
          </div>
        </div>

        {result && (
          <div className={styles.resultCard}>
            <h2 className={styles.resultTitle}>Result</h2>
            <div className={styles.resultDetails}>
              <div className={styles.resultRow}>
                <span className={styles.resultLabel}>Action ID</span>
                <span className={styles.resultActionId}>{result.action_id}</span>
              </div>
              <div className={styles.resultRow}>
                <span className={styles.resultLabel}>Outcome</span>
                <Badge>{result.outcome}</Badge>
              </div>
            </div>
            <div className={styles.resultDetailsSection}>
              <span className={styles.resultDetailsLabel}>Details:</span>
              <div className={styles.jsonViewerCard}>
                <JsonViewer data={result.details} />
              </div>
            </div>
            <div className={styles.viewAuditContainer}>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => navigate(`/audit?action_id=${result.action_id}`)}
              >
                View in Audit Trail
              </Button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
