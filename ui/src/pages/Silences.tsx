import { useMemo, useState } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import { BellOff, Plus, Trash2, X } from 'lucide-react'
import {
  useSilences,
  useCreateSilence,
  useExpireSilence,
  useUpdateSilence,
} from '../api/hooks/useSilences'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Modal } from '../components/ui/Modal'
import { Drawer } from '../components/ui/Drawer'
import { Toggle } from '../components/ui/Toggle'
import { useToast } from '../components/ui/useToast'
import { formatCountdown, relativeTime } from '../lib/format'
import type {
  CreateSilenceRequest,
  Silence,
  SilenceMatcher,
  SilenceMatchOp,
} from '../types'
import shared from '../styles/shared.module.css'
import styles from './Silences.module.css'

const OP_LABEL: Record<SilenceMatchOp, string> = {
  equal: '=',
  not_equal: '!=',
  regex: '=~',
  not_regex: '!~',
}

const OP_OPTIONS: { value: SilenceMatchOp; label: string }[] = [
  { value: 'equal', label: 'equals (=)' },
  { value: 'not_equal', label: 'not equals (!=)' },
  { value: 'regex', label: 'regex (=~)' },
  { value: 'not_regex', label: 'not regex (!~)' },
]

const DURATION_PRESETS: { value: number; label: string }[] = [
  { value: 15 * 60, label: '15 minutes' },
  { value: 60 * 60, label: '1 hour' },
  { value: 2 * 60 * 60, label: '2 hours' },
  { value: 4 * 60 * 60, label: '4 hours' },
  { value: 8 * 60 * 60, label: '8 hours' },
  { value: 24 * 60 * 60, label: '1 day' },
  { value: 7 * 24 * 60 * 60, label: '1 week' },
]

function MatchersSummary({ matchers }: { matchers: SilenceMatcher[] }) {
  return (
    <div className={styles.matchersSummary}>
      {matchers.map((m, i) => (
        <span key={i} className={styles.matcherPill}>
          <span>{m.name}</span>
          <span className={styles.matcherOp}>{OP_LABEL[m.op]}</span>
          <span>"{m.value}"</span>
        </span>
      ))}
    </div>
  )
}

const col = createColumnHelper<Silence>()

export function Silences() {
  const { toast } = useToast()

  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')
  const [includeExpired, setIncludeExpired] = useState(false)

  const [showCreate, setShowCreate] = useState(false)
  const [selected, setSelected] = useState<Silence | null>(null)
  const [expireTarget, setExpireTarget] = useState<Silence | null>(null)

  const { data: silences, isLoading } = useSilences({
    namespace: ns || undefined,
    tenant: tenant || undefined,
    includeExpired,
  })

  const createMutation = useCreateSilence()
  const updateMutation = useUpdateSilence()
  const expireMutation = useExpireSilence()

  const handleExpire = () => {
    if (!expireTarget) return
    expireMutation.mutate(expireTarget.id, {
      onSuccess: () => {
        toast('success', 'Silence expired')
        setExpireTarget(null)
        if (selected?.id === expireTarget.id) setSelected(null)
      },
      onError: (e) => toast('error', 'Expire failed', (e as Error).message),
    })
  }

  const columns = useMemo(
    () => [
      col.accessor('active', {
        header: 'Status',
        cell: (info) => (
          <Badge variant={info.getValue() ? 'success' : 'neutral'} size="sm">
            {info.getValue() ? 'Active' : 'Expired'}
          </Badge>
        ),
      }),
      col.accessor('matchers', {
        header: 'Matchers',
        cell: (info) => <MatchersSummary matchers={info.getValue()} />,
      }),
      col.accessor('tenant', {
        header: 'Tenant',
        cell: (info) => <span className={shared.detailValue}>{info.getValue()}</span>,
      }),
      col.accessor('namespace', {
        header: 'Namespace',
        cell: (info) => <span className={shared.detailValue}>{info.getValue()}</span>,
      }),
      col.accessor('comment', {
        header: 'Comment',
        cell: (info) => <span className={styles.commentCell}>{info.getValue()}</span>,
      }),
      col.accessor('ends_at', {
        header: 'Ends',
        cell: (info) => {
          const row = info.row.original
          return (
            <span className={styles.expiryTime}>
              {row.active ? formatCountdown(info.getValue()) : relativeTime(info.getValue())}
            </span>
          )
        },
      }),
      col.accessor('created_by', {
        header: 'Created by',
        cell: (info) => <span className={shared.detailValue}>{info.getValue()}</span>,
      }),
      col.display({
        id: 'actions',
        header: '',
        cell: (info) => {
          const row = info.row.original
          if (!row.active) return null
          return (
            <div
              className={shared.actionsCell}
              onClick={(e) => e.stopPropagation()}
              role="group"
              aria-label="Row actions"
            >
              <Button
                variant="ghost"
                size="sm"
                icon={<X className="h-3.5 w-3.5" />}
                onClick={() => setExpireTarget(row)}
                aria-label="Expire silence"
              >
                Expire
              </Button>
            </div>
          )
        },
      }),
    ],
    [],
  )

  return (
    <div>
      <PageHeader
        title="Silences"
        subtitle="Time-bounded label matchers that suppress dispatched actions"
        actions={
          <Button
            icon={<Plus className="h-3.5 w-3.5" />}
            onClick={() => setShowCreate(true)}
          >
            Create Silence
          </Button>
        }
      />

      <div className={styles.filterRow}>
        <Input
          placeholder="Namespace"
          value={ns}
          onChange={(e) => setNs(e.target.value)}
        />
        <Input
          placeholder="Tenant"
          value={tenant}
          onChange={(e) => setTenant(e.target.value)}
        />
        <label className={styles.toggleLabel}>
          <Toggle checked={includeExpired} onChange={setIncludeExpired} label="Include expired" />
          Include expired
        </label>
      </div>

      <DataTable
        data={silences ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={setSelected}
        emptyTitle="No silences"
        emptyDescription={
          includeExpired
            ? 'No silences match your filters.'
            : 'No active silences. Create one to mute dispatches during a maintenance window or incident.'
        }
      />

      <SilenceCreateModal
        open={showCreate}
        onClose={() => setShowCreate(false)}
        loading={createMutation.isPending}
        onSubmit={(req) =>
          createMutation.mutate(req, {
            onSuccess: (res) => {
              toast('success', 'Silence created', `ID: ${res.id.slice(0, 12)}…`)
              setShowCreate(false)
            },
            onError: (e) => toast('error', 'Create failed', (e as Error).message),
          })
        }
      />

      <Drawer
        open={!!selected}
        onClose={() => setSelected(null)}
        title={selected ? `Silence ${selected.id.slice(0, 12)}…` : ''}
        wide
        footer={
          selected && selected.active ? (
            <Button
              variant="danger"
              size="sm"
              icon={<BellOff className="h-3.5 w-3.5" />}
              onClick={() => setExpireTarget(selected)}
            >
              Expire silence
            </Button>
          ) : undefined
        }
      >
        {selected && (
          <SilenceDetail
            silence={selected}
            onExtend={(minutes) => {
              const newEnd = new Date(Date.now() + minutes * 60 * 1000).toISOString()
              updateMutation.mutate(
                { id: selected.id, body: { ends_at: newEnd } },
                {
                  onSuccess: (res) => {
                    toast('success', 'Silence extended')
                    setSelected(res)
                  },
                  onError: (e) =>
                    toast('error', 'Extend failed', (e as Error).message),
                },
              )
            }}
            extending={updateMutation.isPending}
          />
        )}
      </Drawer>

      <Modal
        open={!!expireTarget}
        onClose={() => setExpireTarget(null)}
        title="Expire silence"
        size="sm"
        footer={
          <>
            <Button variant="secondary" onClick={() => setExpireTarget(null)}>
              Cancel
            </Button>
            <Button
              variant="danger"
              loading={expireMutation.isPending}
              onClick={handleExpire}
              icon={<BellOff className="h-3.5 w-3.5" />}
            >
              Expire silence
            </Button>
          </>
        }
      >
        <p className={shared.deleteWarning}>
          Expire silence for{' '}
          <span className={shared.deleteName}>
            {expireTarget ? `${expireTarget.tenant}/${expireTarget.namespace}` : ''}
          </span>
          ? Matching dispatches will resume immediately. The audit record stays
          queryable.
        </p>
      </Modal>
    </div>
  )
}

// ---- Detail panel ----

function SilenceDetail({
  silence,
  onExtend,
  extending,
}: {
  silence: Silence
  onExtend: (minutesFromNow: number) => void
  extending: boolean
}) {
  const [extendMinutes, setExtendMinutes] = useState('60')

  return (
    <div className={styles.detailContent}>
      <div className={shared.detailRow}>
        <span className={shared.detailLabel}>Status</span>
        <Badge variant={silence.active ? 'success' : 'neutral'}>
          {silence.active ? 'Active' : 'Expired'}
        </Badge>
      </div>
      <div className={shared.detailRow}>
        <span className={shared.detailLabel}>ID</span>
        <span className={styles.detailValueWrap}>{silence.id}</span>
      </div>
      <div className={shared.detailRow}>
        <span className={shared.detailLabel}>Tenant</span>
        <span className={styles.detailValueWrap}>{silence.tenant}</span>
      </div>
      <div className={shared.detailRow}>
        <span className={shared.detailLabel}>Namespace</span>
        <span className={styles.detailValueWrap}>{silence.namespace}</span>
      </div>
      <div className={shared.detailRow}>
        <span className={shared.detailLabel}>Created by</span>
        <span className={styles.detailValueWrap}>{silence.created_by}</span>
      </div>
      <div className={shared.detailRow}>
        <span className={shared.detailLabel}>Comment</span>
        <span className={styles.detailValueWrap}>{silence.comment || '—'}</span>
      </div>
      <div className={shared.detailRow}>
        <span className={shared.detailLabel}>Starts</span>
        <span className={styles.detailValueWrap}>{relativeTime(silence.starts_at)}</span>
      </div>
      <div className={shared.detailRow}>
        <span className={shared.detailLabel}>Ends</span>
        <span className={styles.detailValueWrap}>
          {silence.active ? formatCountdown(silence.ends_at) : relativeTime(silence.ends_at)}
        </span>
      </div>
      <div className={shared.detailRow}>
        <span className={shared.detailLabel}>Created</span>
        <span className={styles.detailValueWrap}>{relativeTime(silence.created_at)}</span>
      </div>
      <div className={shared.detailRow}>
        <span className={shared.detailLabel}>Updated</span>
        <span className={styles.detailValueWrap}>{relativeTime(silence.updated_at)}</span>
      </div>

      <h3 className={styles.sectionTitle}>Matchers (AND)</h3>
      <MatchersSummary matchers={silence.matchers} />

      {silence.active && (
        <>
          <h3 className={styles.sectionTitle}>Extend silence</h3>
          <div className={styles.matcherRow}>
            <div className={styles.matcherRowInput}>
              <Input
                type="number"
                min="1"
                value={extendMinutes}
                onChange={(e) => setExtendMinutes(e.target.value)}
                placeholder="Minutes from now"
              />
            </div>
            <Button
              size="sm"
              loading={extending}
              onClick={() => {
                const m = parseInt(extendMinutes, 10)
                if (Number.isFinite(m) && m > 0) onExtend(m)
              }}
            >
              Extend
            </Button>
          </div>
        </>
      )}
    </div>
  )
}

// ---- Create modal ----

function SilenceCreateModal({
  open,
  onClose,
  onSubmit,
  loading,
}: {
  open: boolean
  onClose: () => void
  onSubmit: (req: CreateSilenceRequest) => void
  loading: boolean
}) {
  const [namespace, setNamespace] = useState('')
  const [tenant, setTenant] = useState('')
  const [comment, setComment] = useState('')
  const [duration, setDuration] = useState('3600')
  const [matchers, setMatchers] = useState<SilenceMatcher[]>([
    { name: '', value: '', op: 'equal' },
  ])

  const handleMatcherChange = (index: number, patch: Partial<SilenceMatcher>) => {
    setMatchers((prev) =>
      prev.map((m, i) => (i === index ? { ...m, ...patch } : m)),
    )
  }

  const addMatcher = () =>
    setMatchers((prev) => [...prev, { name: '', value: '', op: 'equal' }])

  const removeMatcher = (index: number) =>
    setMatchers((prev) => prev.filter((_, i) => i !== index))

  const reset = () => {
    setNamespace('')
    setTenant('')
    setComment('')
    setDuration('3600')
    setMatchers([{ name: '', value: '', op: 'equal' }])
  }

  const handleClose = () => {
    reset()
    onClose()
  }

  const validMatchers = matchers.filter((m) => m.name.trim() && m.value.trim())
  const isValid =
    namespace.trim() !== '' &&
    tenant.trim() !== '' &&
    comment.trim() !== '' &&
    validMatchers.length > 0 &&
    Number.parseInt(duration, 10) > 0

  return (
    <Modal
      open={open}
      onClose={handleClose}
      title="Create silence"
      size="lg"
      footer={
        <>
          <Button variant="secondary" onClick={handleClose}>
            Cancel
          </Button>
          <Button
            loading={loading}
            disabled={!isValid}
            icon={<BellOff className="h-3.5 w-3.5" />}
            onClick={() =>
              onSubmit({
                namespace: namespace.trim(),
                tenant: tenant.trim(),
                comment: comment.trim(),
                duration_seconds: Number.parseInt(duration, 10),
                matchers: validMatchers,
              })
            }
          >
            Silence
          </Button>
        </>
      }
    >
      <div className={shared.formSection}>
        <div className={shared.formGrid}>
          <Input
            label="Namespace *"
            value={namespace}
            onChange={(e) => setNamespace(e.target.value)}
            placeholder="prod"
          />
          <Input
            label="Tenant *"
            value={tenant}
            onChange={(e) => setTenant(e.target.value)}
            placeholder="acme"
          />
        </div>

        <Select
          label="Duration *"
          value={duration}
          onChange={(e) => setDuration(e.target.value)}
          options={DURATION_PRESETS.map((p) => ({
            value: String(p.value),
            label: p.label,
          }))}
        />

        <Input
          label="Comment *"
          value={comment}
          onChange={(e) => setComment(e.target.value)}
          placeholder="canary deploy, expected 5xx"
        />

        <div>
          <div className={shared.textareaLabel}>Matchers (AND) *</div>
          <div className={styles.matcherList}>
            {matchers.map((m, i) => (
              <div key={i} className={styles.matcherRow}>
                <div className={styles.matcherRowInput}>
                  <Input
                    value={m.name}
                    onChange={(e) => handleMatcherChange(i, { name: e.target.value })}
                    placeholder="label name (e.g. severity)"
                  />
                </div>
                <Select
                  value={m.op}
                  onChange={(e) =>
                    handleMatcherChange(i, { op: e.target.value as SilenceMatchOp })
                  }
                  options={OP_OPTIONS}
                />
                <div className={styles.matcherRowInput}>
                  <Input
                    value={m.value}
                    onChange={(e) =>
                      handleMatcherChange(i, { value: e.target.value })
                    }
                    placeholder={
                      m.op === 'regex' || m.op === 'not_regex'
                        ? 'regex pattern'
                        : 'value'
                    }
                  />
                </div>
                {matchers.length > 1 && (
                  <Button
                    variant="ghost"
                    size="sm"
                    icon={<Trash2 className="h-3.5 w-3.5" />}
                    onClick={() => removeMatcher(i)}
                    aria-label="Remove matcher"
                  />
                )}
              </div>
            ))}
          </div>
          <div className={styles.addMatcherSpacing}>
            <Button
              variant="secondary"
              size="sm"
              icon={<Plus className="h-3.5 w-3.5" />}
              onClick={addMatcher}
            >
              Add matcher
            </Button>
          </div>
        </div>
      </div>
    </Modal>
  )
}
