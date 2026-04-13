import { useMemo, useState } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import { Plus, Trash2 } from 'lucide-react'
import {
  useTimeIntervals,
  useCreateTimeInterval,
  useDeleteTimeInterval,
} from '../api/hooks/useTimeIntervals'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Modal } from '../components/ui/Modal'
import { useToast } from '../components/ui/useToast'
import type { CreateTimeIntervalRequest, TimeInterval, TimeRange } from '../types'
import shared from '../styles/shared.module.css'
import styles from './Silences.module.css'

const col = createColumnHelper<TimeInterval>()

function summarizeRanges(ranges: TimeRange[]): string {
  if (!ranges || ranges.length === 0) return '—'
  return ranges
    .map((r) => {
      const parts: string[] = []
      if (r.times && r.times.length > 0) {
        parts.push(r.times.map((t) => `${t.start}-${t.end}`).join(','))
      }
      if (r.weekdays && r.weekdays.length > 0) {
        parts.push(`wd ${r.weekdays.map((w) => `${w.start}-${w.end}`).join(',')}`)
      }
      if (r.days_of_month && r.days_of_month.length > 0) {
        parts.push(`dom ${r.days_of_month.map((d) => `${d.start}-${d.end}`).join(',')}`)
      }
      if (r.months && r.months.length > 0) {
        parts.push(`mo ${r.months.map((m) => `${m.start}-${m.end}`).join(',')}`)
      }
      if (parts.length === 0) return '*'
      return parts.join(' & ')
    })
    .join(' | ')
}

export function TimeIntervals() {
  const { toast } = useToast()
  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')
  const [showCreate, setShowCreate] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<TimeInterval | null>(null)

  const { data: intervals, isLoading } = useTimeIntervals({
    namespace: ns || undefined,
    tenant: tenant || undefined,
  })

  const createMutation = useCreateTimeInterval()
  const deleteMutation = useDeleteTimeInterval()

  const handleDelete = () => {
    if (!deleteTarget) return
    deleteMutation.mutate(
      {
        namespace: deleteTarget.namespace,
        tenant: deleteTarget.tenant,
        name: deleteTarget.name,
      },
      {
        onSuccess: () => {
          toast('success', 'Time interval deleted')
          setDeleteTarget(null)
        },
        onError: (e) => toast('error', 'Delete failed', (e as Error).message),
      },
    )
  }

  const columns = useMemo(
    () => [
      col.accessor('matches_now', {
        header: 'Now',
        cell: (info) => (
          <Badge variant={info.getValue() ? 'success' : 'neutral'} size="sm">
            {info.getValue() ? 'Matching' : 'Idle'}
          </Badge>
        ),
      }),
      col.accessor('name', {
        header: 'Name',
        cell: (info) => <span className={shared.detailValue}>{info.getValue()}</span>,
      }),
      col.accessor('namespace', {
        header: 'Namespace',
        cell: (info) => <span className={shared.detailValue}>{info.getValue()}</span>,
      }),
      col.accessor('tenant', {
        header: 'Tenant',
        cell: (info) => <span className={shared.detailValue}>{info.getValue()}</span>,
      }),
      col.accessor('time_ranges', {
        header: 'Ranges',
        cell: (info) => (
          <span className={styles.commentCell}>{summarizeRanges(info.getValue())}</span>
        ),
      }),
      col.accessor('location', {
        header: 'Location',
        cell: (info) => (
          <span className={shared.detailValue}>{info.getValue() ?? 'UTC'}</span>
        ),
      }),
      col.display({
        id: 'actions',
        header: '',
        cell: (info) => (
          <div
            className={shared.actionsCell}
            onClick={(e) => e.stopPropagation()}
            role="group"
          >
            <Button
              variant="ghost"
              size="sm"
              icon={<Trash2 className="h-3.5 w-3.5" />}
              onClick={() => setDeleteTarget(info.row.original)}
              aria-label="Delete time interval"
            >
              Delete
            </Button>
          </div>
        ),
      }),
    ],
    [],
  )

  return (
    <div>
      <PageHeader
        title="Time Intervals"
        subtitle="Recurring schedules that rules reference to mute or activate dispatches"
        actions={
          <Button
            icon={<Plus className="h-3.5 w-3.5" />}
            onClick={() => setShowCreate(true)}
          >
            Create Interval
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
      </div>

      <DataTable
        data={intervals ?? []}
        columns={columns}
        loading={isLoading}
        emptyTitle="No time intervals"
        emptyDescription="Create a time interval and reference it from a rule via mute_time_intervals or active_time_intervals to gate dispatch by wall-clock time."
      />

      <TimeIntervalCreateModal
        open={showCreate}
        onClose={() => setShowCreate(false)}
        loading={createMutation.isPending}
        onSubmit={(req) =>
          createMutation.mutate(req, {
            onSuccess: (res) => {
              toast('success', 'Time interval created', res.name)
              setShowCreate(false)
            },
            onError: (e) => toast('error', 'Create failed', (e as Error).message),
          })
        }
      />

      <Modal
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        title="Delete time interval"
        footer={
          <>
            <Button variant="ghost" onClick={() => setDeleteTarget(null)}>
              Cancel
            </Button>
            <Button
              variant="danger"
              onClick={handleDelete}
              loading={deleteMutation.isPending}
            >
              Delete
            </Button>
          </>
        }
      >
        <p>
          Delete time interval <strong>{deleteTarget?.name}</strong>? Rules that still
          reference it will treat it as "not found" and proceed without gating.
        </p>
      </Modal>
    </div>
  )
}

const EXAMPLE_RANGES = `[
  {
    "times": [{ "start": "09:00", "end": "17:00" }],
    "weekdays": [{ "start": 1, "end": 5 }]
  }
]`

function TimeIntervalCreateModal({
  open,
  onClose,
  onSubmit,
  loading,
}: {
  open: boolean
  onClose: () => void
  onSubmit: (req: CreateTimeIntervalRequest) => void
  loading: boolean
}) {
  const [name, setName] = useState('')
  const [namespace, setNamespace] = useState('prod')
  const [tenant, setTenant] = useState('default')
  const [location, setLocation] = useState('')
  const [description, setDescription] = useState('')
  const [rangesJson, setRangesJson] = useState(EXAMPLE_RANGES)
  const [error, setError] = useState<string | null>(null)

  const handleSubmit = () => {
    setError(null)
    let time_ranges: TimeRange[]
    try {
      time_ranges = JSON.parse(rangesJson)
      if (!Array.isArray(time_ranges)) {
        throw new Error('time_ranges must be a JSON array')
      }
    } catch (e) {
      setError(`Invalid JSON: ${(e as Error).message}`)
      return
    }
    onSubmit({
      name,
      namespace,
      tenant,
      time_ranges,
      location: location || undefined,
      description: description || undefined,
    })
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Create time interval"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} loading={loading} disabled={!name}>
            Create
          </Button>
        </>
      }
    >
      <div style={{ display: 'flex', flexDirection: 'column', gap: '0.75rem' }}>
        <Input
          label="Name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="business-hours"
          required
        />
        <Input
          label="Namespace"
          value={namespace}
          onChange={(e) => setNamespace(e.target.value)}
        />
        <Input
          label="Tenant"
          value={tenant}
          onChange={(e) => setTenant(e.target.value)}
        />
        <Input
          label="Location (IANA timezone, optional)"
          value={location}
          onChange={(e) => setLocation(e.target.value)}
          placeholder="America/New_York"
        />
        <Input
          label="Description (optional)"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
        />
        <label className={shared.detailLabel}>Time ranges (JSON)</label>
        <textarea
          value={rangesJson}
          onChange={(e) => setRangesJson(e.target.value)}
          rows={10}
          style={{
            fontFamily: 'monospace',
            fontSize: '0.85rem',
            padding: '0.5rem',
            border: '1px solid var(--border-default)',
            borderRadius: '4px',
            background: 'var(--bg-surface)',
            color: 'var(--text-primary)',
          }}
        />
        {error && (
          <div style={{ color: 'var(--text-danger)', fontSize: '0.85rem' }}>{error}</div>
        )}
      </div>
    </Modal>
  )
}
