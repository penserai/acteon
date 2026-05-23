import { useState, useCallback } from 'react'
import { Play, AlertTriangle } from 'lucide-react'
import {
  useTemplateProfiles,
  useRenderPreview,
} from '../../api/hooks/useTemplates'
import { Button } from '../../components/ui/Button'
import { Input } from '../../components/ui/Input'
import { useToast } from '../../components/ui/useToast'
import { tryParseJson } from '../../lib/format'
import type { RenderPreviewResponse } from '../../types'
import shared from '../../styles/shared.module.css'
import styles from '../Templates.module.css'

export function PlaygroundTab() {
  const { toast } = useToast()
  const renderMutation = useRenderPreview()

  // Form state
  const [profile, setProfile] = useState('')
  const [ns, setNs] = useState('default')
  const [tenant, setTenant] = useState('test-tenant')
  const [payload, setPayload] = useState('{\n  "name": "Jane Doe",\n  "email": "jane@example.com",\n  "amount": 99.50\n}')

  // Validation
  const [payloadError, setPayloadError] = useState('')

  // Result
  const [result, setResult] = useState<RenderPreviewResponse | null>(null)
  const [renderError, setRenderError] = useState('')

  // Load available profiles for the selected scope
  const { data: profilesData } = useTemplateProfiles({
    namespace: ns || undefined,
    tenant: tenant || undefined,
  })
  const availableProfiles = profilesData?.profiles ?? []

  const handleRender = useCallback(() => {
    setRenderError('')
    setResult(null)

    if (!profile.trim()) {
      toast('error', 'Profile name is required')
      return
    }

    const parsed = tryParseJson(payload)
    if (!parsed.ok) {
      setPayloadError(parsed.error)
      return
    }
    setPayloadError('')

    renderMutation.mutate(
      {
        profile: profile.trim(),
        namespace: ns,
        tenant,
        payload: parsed.value,
      },
      {
        onSuccess: (data) => {
          setResult(data)
          setRenderError('')
        },
        onError: (e) => {
          setRenderError((e as Error).message)
          toast('error', 'Render failed', (e as Error).message)
        },
      },
    )
  }, [profile, ns, tenant, payload, renderMutation, toast])

  const isValid = profile.trim() && ns.trim() && tenant.trim() && !payloadError

  return (
    <div className={styles.playgroundContainer}>
      <div className={styles.playgroundLayout}>
        {/* Left: form inputs */}
        <div className={styles.playgroundCard}>
          <h3 className={styles.playgroundCardTitle}>Configuration</h3>

          <div className={shared.formGrid}>
            <Input
              label="Namespace"
              value={ns}
              onChange={(e) => setNs(e.target.value)}
              placeholder="default"
            />
            <Input
              label="Tenant"
              value={tenant}
              onChange={(e) => setTenant(e.target.value)}
              placeholder="test-tenant"
            />
          </div>

          <div>
            <label className={shared.textareaLabel} htmlFor="playground-profile">
              Profile
            </label>
            {availableProfiles.length > 0 ? (
              <select
                id="playground-profile"
                className={styles.selectInput}
                value={profile}
                onChange={(e) => setProfile(e.target.value)}
              >
                <option value="">Select a profile...</option>
                {availableProfiles.map((p) => (
                  <option key={p.id} value={p.name}>
                    {p.name}
                    {p.description ? ` — ${p.description}` : ''}
                  </option>
                ))}
              </select>
            ) : (
              <Input
                value={profile}
                onChange={(e) => setProfile(e.target.value)}
                placeholder="profile-name"
              />
            )}
          </div>

          <div>
            <label className={shared.textareaLabel} htmlFor="playground-payload">
              Payload (JSON variables)
            </label>
            <textarea
              id="playground-payload"
              className={styles.playgroundTextarea}
              value={payload}
              onChange={(e) => {
                setPayload(e.target.value)
                if (payloadError) {
                  const check = tryParseJson(e.target.value)
                  if (check.ok) setPayloadError('')
                }
              }}
              spellCheck={false}
            />
            {payloadError && (
              <p className={styles.validationError}>{payloadError}</p>
            )}
          </div>

          <div className={styles.playgroundSubmit}>
            <Button
              icon={<Play className="h-3.5 w-3.5" />}
              onClick={handleRender}
              loading={renderMutation.isPending}
              disabled={!isValid}
            >
              Render Preview
            </Button>
          </div>
        </div>

        {/* Right: results */}
        <div className={styles.playgroundCard}>
          <h3 className={styles.playgroundCardTitle}>Rendered Output</h3>

          {renderError && (
            <div className={styles.playgroundError}>
              <AlertTriangle className="h-4 w-4 flex-none" />
              <span>{renderError}</span>
            </div>
          )}

          {result && Object.keys(result.rendered).length > 0 ? (
            <div className={styles.renderedFields} role="list" aria-label="Rendered fields">
              {Object.entries(result.rendered).map(([field, value]) => (
                <div key={field} className={styles.renderedField} role="listitem">
                  <div className={styles.renderedFieldHeader}>
                    <span className={styles.renderedFieldName}>{field}</span>
                  </div>
                  <pre className={styles.renderedFieldValue}>{value}</pre>
                </div>
              ))}
            </div>
          ) : !renderError ? (
            <p className={styles.playgroundEmpty}>
              Select a profile and provide payload variables, then click
              &quot;Render Preview&quot; to see the rendered output for each field.
            </p>
          ) : null}
        </div>
      </div>
    </div>
  )
}
