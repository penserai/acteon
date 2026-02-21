import { useState, useMemo, useCallback, useRef } from 'react'
import { useNavigate } from 'react-router-dom'
import { Send, Paperclip, X } from 'lucide-react'
import { useDispatch } from '../api/hooks/useActions'
import { useConfig } from '../api/hooks/useConfig'
import { useAudit } from '../api/hooks/useAudit'
import { PageHeader } from '../components/layout/PageHeader'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Button } from '../components/ui/Button'
import { Badge } from '../components/ui/Badge'
import { JsonViewer } from '../components/ui/JsonViewer'
import { useToast } from '../components/ui/useToast'
import type { Attachment, DispatchRequest, DispatchResponse } from '../types'
import styles from './Dispatch.module.css'

/**
 * Known action types keyed by provider type AND provider name.
 * Lookup checks provider name first, then provider type, then merges both.
 */
const ACTION_TYPES: Record<string, string[]> = {
  // By provider type
  email: ['send_email'],
  slack: ['send_message'],
  twilio: ['send_sms'],
  teams: ['notify'],
  discord: ['notify'],
  pagerduty: ['trigger', 'acknowledge', 'resolve'],
  webhook: ['send'],
  log: ['send'],
  'aws-sns': ['publish'],
  'aws-lambda': ['invoke'],
  'aws-eventbridge': ['put_event'],
  'aws-sqs': ['send_message'],
  'aws-s3': ['put_object', 'get_object', 'delete_object'],
  'aws-ec2': [
    'run_instances', 'start_instances', 'stop_instances',
    'reboot_instances', 'terminate_instances', 'hibernate_instances',
    'describe_instances', 'attach_volume', 'detach_volume',
  ],
  'aws-autoscaling': [
    'describe_auto_scaling_groups', 'set_desired_capacity',
    'update_auto_scaling_group',
  ],
  // By common provider name (when name differs from type, e.g. name="sms" type="log")
  sms: ['send_sms'],
}

const CUSTOM = '__custom__'

/** Select that falls back to an Input when "Custom..." is chosen or no options exist. */
function SelectOrCustom({
  label,
  value,
  onChange,
  options,
  placeholder,
}: {
  label: string
  value: string
  onChange: (v: string) => void
  options: string[]
  placeholder: string
}) {
  const [custom, setCustom] = useState(false)

  if (custom || options.length === 0) {
    return (
      <div>
        <Input
          label={label}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
        />
        {options.length > 0 && (
          <button
            type="button"
            className={styles.switchLink}
            onClick={() => { setCustom(false); onChange('') }}
          >
            Pick from list
          </button>
        )}
      </div>
    )
  }

  const selectOptions = [
    ...options.map((o) => ({ value: o, label: o })),
    { value: CUSTOM, label: 'Custom...' },
  ]

  return (
    <Select
      label={label}
      options={selectOptions}
      value={value}
      onChange={(e) => {
        if (e.target.value === CUSTOM) {
          setCustom(true)
          onChange('')
        } else {
          onChange(e.target.value)
        }
      }}
      placeholder={placeholder}
    />
  )
}

export function Dispatch() {
  const dispatch = useDispatch()
  const { toast } = useToast()
  const navigate = useNavigate()
  const config = useConfig()
  const audit = useAudit({ limit: 200 })
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')
  const [provider, setProvider] = useState('')
  const [actionType, setActionType] = useState('')
  const [payload, setPayload] = useState('{\n  \n}')
  const [dedupKey, setDedupKey] = useState('')
  const [dryRun, setDryRun] = useState(false)
  const [result, setResult] = useState<DispatchResponse | null>(null)
  const [payloadError, setPayloadError] = useState('')
  const [attachments, setAttachments] = useState<{ id: string; name: string; file: File; base64: string }[]>([])
  const fileInputRef = useRef<HTMLInputElement>(null)

  const handleFileSelect = useCallback(async (files: FileList | null) => {
    if (!files) return
    const newAttachments: { id: string; name: string; file: File; base64: string }[] = []
    for (const file of Array.from(files)) {
      const buffer = await file.arrayBuffer()
      const base64 = btoa(
        new Uint8Array(buffer).reduce((s, b) => s + String.fromCharCode(b), ''),
      )
      newAttachments.push({
        id: crypto.randomUUID(),
        name: file.name.replace(/\.[^.]+$/, ''),
        file,
        base64,
      })
    }
    setAttachments((prev) => [...prev, ...newAttachments])
  }, [])

  const removeAttachment = useCallback((index: number) => {
    setAttachments((prev) => prev.filter((_, i) => i !== index))
  }, [])

  const providers = useMemo(() => config.data?.providers ?? [], [config.data])

  const providerOptions = useMemo(() =>
    providers.map((p) => ({
      value: p.name,
      label: `${p.name} (${p.provider_type})`,
    })),
    [providers],
  )

  // Resolve the selected provider's type to look up action types.
  const selectedProviderType = useMemo(() => {
    const p = providers.find((pr) => pr.name === provider)
    return p?.provider_type ?? ''
  }, [providers, provider])

  // Action types: merge by provider name + provider type + audit history.
  const actionTypeOptions = useMemo(() => {
    const byName = ACTION_TYPES[provider] ?? []
    const byType = ACTION_TYPES[selectedProviderType] ?? []
    const fromAudit = new Set<string>()
    if (audit.data?.records) {
      for (const r of audit.data.records) {
        if (r.provider === provider) {
          fromAudit.add(r.action_type)
        }
      }
    }
    const merged = new Set([...byName, ...byType, ...fromAudit])
    return [...merged].sort()
  }, [selectedProviderType, provider, audit.data])

  const { namespaces, tenants } = useMemo(() => {
    const nsSet = new Set<string>()
    const tenantSet = new Set<string>()
    if (audit.data?.records) {
      for (const r of audit.data.records) {
        nsSet.add(r.namespace)
        tenantSet.add(r.tenant)
      }
    }
    return {
      namespaces: [...nsSet].sort(),
      tenants: [...tenantSet].sort(),
    }
  }, [audit.data])

  // Reset action type when provider changes.
  const handleProviderChange = (v: string) => {
    setProvider(v)
    setActionType('')
  }

  const handleDispatch = () => {
    let parsed: Record<string, unknown>
    try {
      parsed = JSON.parse(payload)
      setPayloadError('')
    } catch {
      setPayloadError('Invalid JSON')
      return
    }

    const flatAttachments: Attachment[] = attachments.map((a) => ({
      id: a.id,
      name: a.name,
      filename: a.file.name,
      content_type: a.file.type || 'application/octet-stream',
      data_base64: a.base64,
    }))

    const request: DispatchRequest = {
      namespace: ns,
      tenant,
      provider,
      action_type: actionType,
      payload: parsed,
      dedup_key: dedupKey || undefined,
      attachments: flatAttachments.length > 0 ? flatAttachments : undefined,
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
            <SelectOrCustom
              label="Namespace *"
              value={ns}
              onChange={setNs}
              options={namespaces}
              placeholder="Select namespace"
            />
            <SelectOrCustom
              label="Tenant *"
              value={tenant}
              onChange={setTenant}
              options={tenants}
              placeholder="Select tenant"
            />
            <Select
              label="Provider *"
              options={providerOptions}
              value={provider}
              onChange={(e) => handleProviderChange(e.target.value)}
              placeholder="Select a provider"
            />
            <SelectOrCustom
              label="Action Type *"
              value={actionType}
              onChange={setActionType}
              options={actionTypeOptions}
              placeholder="Select action type"
            />
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

          <div>
            <label className={styles.textareaLabel}>Attachments</label>
            <input
              ref={fileInputRef}
              type="file"
              multiple
              className={styles.hiddenInput}
              onChange={(e) => {
                void handleFileSelect(e.target.files)
                e.target.value = ''
              }}
            />
            <button
              type="button"
              className={styles.attachButton}
              onClick={() => fileInputRef.current?.click()}
            >
              <Paperclip className="h-3.5 w-3.5" />
              Add files
            </button>
            {attachments.length > 0 && (
              <ul className={styles.attachmentList}>
                {attachments.map((a, i) => (
                  <li key={a.id} className={styles.attachmentItem}>
                    <div className={styles.attachmentFields}>
                      <input
                        type="text"
                        value={a.id}
                        onChange={(e) => setAttachments(prev => prev.map((att, idx) => idx === i ? { ...att, id: e.target.value } : att))}
                        className={styles.attachmentInput}
                        placeholder="Attachment ID"
                      />
                      <input
                        type="text"
                        value={a.name}
                        onChange={(e) => setAttachments(prev => prev.map((att, idx) => idx === i ? { ...att, name: e.target.value } : att))}
                        className={styles.attachmentInput}
                        placeholder="Display name"
                      />
                    </div>
                    <span className={styles.attachmentName}>{a.file.name}</span>
                    <span className={styles.attachmentSize}>
                      {a.file.size < 1024
                        ? `${a.file.size} B`
                        : a.file.size < 1024 * 1024
                          ? `${(a.file.size / 1024).toFixed(1)} KB`
                          : `${(a.file.size / (1024 * 1024)).toFixed(1)} MB`}
                    </span>
                    <button
                      type="button"
                      className={styles.removeAttachment}
                      onClick={() => removeAttachment(i)}
                      aria-label={`Remove ${a.file.name}`}
                    >
                      <X className="h-3.5 w-3.5" />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>

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
            {result.details && (
              <div className={styles.resultDetailsSection}>
                <span className={styles.resultDetailsLabel}>Details:</span>
                <div className={styles.jsonViewerCard}>
                  <JsonViewer data={result.details} />
                </div>
              </div>
            )}
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
