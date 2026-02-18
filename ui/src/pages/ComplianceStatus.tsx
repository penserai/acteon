import { useState } from 'react'
import {
  ShieldCheck, CheckCircle2, XCircle, Search, Loader2,
} from 'lucide-react'
import { useComplianceStatus, useVerifyChain } from '../api/hooks/useCompliance'
import { PageHeader } from '../components/layout/PageHeader'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { EmptyState } from '../components/ui/EmptyState'
import { Skeleton } from '../components/ui/Skeleton'
import type { ComplianceMode, HashChainVerification } from '../types'
import styles from './ComplianceStatus.module.css'

function modeBadge(mode: ComplianceMode) {
  switch (mode) {
    case 'hipaa':
      return <Badge variant="error" size="md">HIPAA</Badge>
    case 'soc2':
      return <Badge variant="warning" size="md">SOC2</Badge>
    default:
      return <Badge variant="neutral" size="md">None</Badge>
  }
}

function featureIcon(enabled: boolean) {
  if (enabled) {
    return <CheckCircle2 className={styles.featureIconEnabled} />
  }
  return <XCircle className={styles.featureIconDisabled} />
}

export function ComplianceStatus() {
  const { data: status, isLoading, error } = useComplianceStatus()
  const verifyMutation = useVerifyChain()

  const [verifyNs, setVerifyNs] = useState('')
  const [verifyTenant, setVerifyTenant] = useState('')
  const [verification, setVerification] = useState<HashChainVerification | null>(null)

  const handleVerify = () => {
    if (!verifyNs || !verifyTenant) return
    setVerification(null)
    verifyMutation.mutate(
      { namespace: verifyNs, tenant: verifyTenant },
      { onSuccess: (result) => setVerification(result) },
    )
  }

  if (isLoading) {
    return (
      <div>
        <PageHeader title="Compliance Status" />
        <div className={styles.loadingGrid}>
          {Array.from({ length: 4 }).map((_, i) => <Skeleton key={i} className="h-32" />)}
        </div>
      </div>
    )
  }

  if (error) {
    return (
      <div>
        <PageHeader title="Compliance Status" />
        <EmptyState
          icon={<ShieldCheck className="h-12 w-12" />}
          title="Unable to load compliance status"
          description={(error as Error).message}
        />
      </div>
    )
  }

  if (!status) {
    return (
      <div>
        <PageHeader title="Compliance Status" />
        <EmptyState
          icon={<ShieldCheck className="h-12 w-12" />}
          title="No compliance data"
          description="Compliance status will appear once the server is configured."
        />
      </div>
    )
  }

  const features = [
    {
      label: 'Synchronous Audit Writes',
      description: 'Audit writes block the dispatch pipeline until confirmed by the backend.',
      enabled: status.sync_audit_writes,
    },
    {
      label: 'Immutable Audit Records',
      description: 'Audit records cannot be modified or deleted once written.',
      enabled: status.immutable_audit,
    },
    {
      label: 'Hash Chain Integrity',
      description: 'SHA-256 hash chain links audit records within each namespace/tenant pair.',
      enabled: status.hash_chain,
    },
  ]

  return (
    <div>
      <PageHeader
        title="Compliance Status"
        subtitle="View compliance mode configuration and verify audit integrity"
      />

      {/* Mode display */}
      <div className={styles.modeCard}>
        <div className={styles.modeHeader}>
          <ShieldCheck className={styles.modeIcon} />
          <div>
            <div className={styles.modeLabel}>Current Compliance Mode</div>
            <div className={styles.modeBadgeWrap}>{modeBadge(status.mode)}</div>
          </div>
        </div>
      </div>

      {/* Feature status cards */}
      <div className={styles.featuresGrid}>
        {features.map((f) => (
          <div key={f.label} className={styles.featureCard}>
            <div className={styles.featureHeader}>
              {featureIcon(f.enabled)}
              <span className={styles.featureLabel}>{f.label}</span>
            </div>
            <p className={styles.featureDescription}>{f.description}</p>
            <div className={styles.featureStatus}>
              <Badge variant={f.enabled ? 'success' : 'neutral'}>
                {f.enabled ? 'Enabled' : 'Disabled'}
              </Badge>
            </div>
          </div>
        ))}
      </div>

      {/* Chain verification panel */}
      {status.hash_chain && (
        <div className={styles.verifySection}>
          <h2 className={styles.sectionTitle}>Verify Hash Chain</h2>
          <p className={styles.sectionDescription}>
            Check the integrity of the audit hash chain for a specific namespace and tenant.
          </p>

          <div className={styles.verifyForm}>
            <Input
              label="Namespace"
              placeholder="prod"
              value={verifyNs}
              onChange={(e) => setVerifyNs(e.target.value)}
            />
            <Input
              label="Tenant"
              placeholder="acme"
              value={verifyTenant}
              onChange={(e) => setVerifyTenant(e.target.value)}
            />
            <div className={styles.verifyButtonWrap}>
              <Button
                icon={verifyMutation.isPending
                  ? <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  : <Search className="h-3.5 w-3.5" />
                }
                onClick={handleVerify}
                disabled={!verifyNs || !verifyTenant || verifyMutation.isPending}
                loading={verifyMutation.isPending}
              >
                Verify
              </Button>
            </div>
          </div>

          {verifyMutation.isError && (
            <div className={styles.verifyError}>
              Verification failed: {(verifyMutation.error as Error).message}
            </div>
          )}

          {verification && (
            <div className={styles.verifyResult}>
              <div className={styles.verifyResultHeader}>
                {verification.valid ? (
                  <CheckCircle2 className={styles.verifyResultIconValid} />
                ) : (
                  <XCircle className={styles.verifyResultIconInvalid} />
                )}
                <span className={styles.verifyResultTitle}>
                  {verification.valid ? 'Chain integrity verified' : 'Chain integrity broken'}
                </span>
              </div>

              <div className={styles.verifyDetails}>
                <div className={styles.verifyDetailRow}>
                  <span className={styles.verifyDetailLabel}>Records checked</span>
                  <span className={styles.verifyDetailValue}>{verification.records_checked}</span>
                </div>
                {verification.first_record_id && (
                  <div className={styles.verifyDetailRow}>
                    <span className={styles.verifyDetailLabel}>First record</span>
                    <span className={styles.verifyDetailValue}>{verification.first_record_id}</span>
                  </div>
                )}
                {verification.last_record_id && (
                  <div className={styles.verifyDetailRow}>
                    <span className={styles.verifyDetailLabel}>Last record</span>
                    <span className={styles.verifyDetailValue}>{verification.last_record_id}</span>
                  </div>
                )}
                {verification.first_broken_at && (
                  <div className={styles.verifyDetailRow}>
                    <span className={styles.verifyDetailLabel}>First broken at</span>
                    <span className={styles.verifyDetailValueError}>{verification.first_broken_at}</span>
                  </div>
                )}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
