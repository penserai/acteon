import { useState } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import {
  Plus, Pencil, Trash2, ChevronDown, ArrowUp, ArrowDown, GitBranch,
} from 'lucide-react'
import {
  useChainDefinitions,
  useChainDefinition,
  useSaveChainDefinition,
  useDeleteChainDefinition,
} from '../api/hooks/useChainDefinitions'
import { useChainDefinitionDag } from '../api/hooks/useChains'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Modal } from '../components/ui/Modal'
import { DeleteConfirmModal } from '../components/ui/DeleteConfirmModal'
import { Drawer } from '../components/ui/Drawer'
import { Tabs } from '../components/ui/Tabs'
import { useToast } from '../components/ui/useToast'
import type {
  ChainDefinitionSummary,
  ChainDefinition,
  ChainStepConfig,
  BranchCondition,
  ParallelSubStepConfig,
} from '../types'
import shared from '../styles/shared.module.css'
import styles from './ChainDefinitions.module.css'

// ---- Types ----

type StepType = 'provider' | 'sub_chain' | 'parallel'

interface ParallelSubStep {
  name: string
  provider: string
  action_type: string
  payload_template_json: string
  on_failure: string
}

interface StepDraft {
  name: string
  type: StepType
  // Provider step fields
  provider: string
  action_type: string
  payload_template_json: string
  on_failure: string
  delay_seconds: string
  branches: BranchCondition[]
  default_next: string
  // Sub-chain step fields
  sub_chain: string
  // Parallel step fields
  parallel_join: string
  parallel_failure: string
  parallel_timeout: string
  parallel_concurrency: string
  parallel_sub_steps: ParallelSubStep[]
}

// ---- Helpers ----

function newStepDraft(type: StepType = 'provider'): StepDraft {
  return {
    name: '',
    type,
    provider: '',
    action_type: '',
    payload_template_json: '{}',
    on_failure: '',
    delay_seconds: '',
    branches: [],
    default_next: '',
    sub_chain: '',
    parallel_join: 'All',
    parallel_failure: 'FailFast',
    parallel_timeout: '',
    parallel_concurrency: '',
    parallel_sub_steps: [],
  }
}

function newSubStep(): ParallelSubStep {
  return { name: '', provider: '', action_type: '', payload_template_json: '{}', on_failure: '' }
}

function draftToConfig(d: StepDraft): ChainStepConfig {
  if (d.type === 'sub_chain') {
    return {
      name: d.name,
      provider: '',
      action_type: '',
      payload_template: {},
      branches: [],
      sub_chain: d.sub_chain || undefined,
    }
  }
  if (d.type === 'parallel') {
    let payload: Record<string, unknown> = {}
    try { payload = JSON.parse(d.payload_template_json) } catch { /* ignore */ }
    return {
      name: d.name,
      provider: '',
      action_type: '',
      payload_template: payload,
      branches: [],
      parallel: {
        steps: d.parallel_sub_steps.map((s) => {
          let sPayload: Record<string, unknown> = {}
          try { sPayload = JSON.parse(s.payload_template_json) } catch { /* ignore */ }
          return {
            name: s.name,
            provider: s.provider,
            action_type: s.action_type,
            payload_template: sPayload,
            on_failure: s.on_failure || undefined,
            branches: [],
          }
        }),
        join: d.parallel_join || 'All',
        on_failure: d.parallel_failure || 'FailFast',
        timeout_seconds: d.parallel_timeout ? Number(d.parallel_timeout) : undefined,
        max_concurrency: d.parallel_concurrency ? Number(d.parallel_concurrency) : undefined,
      },
    }
  }
  // provider
  let payload: Record<string, unknown> = {}
  try { payload = JSON.parse(d.payload_template_json) } catch { /* ignore */ }
  return {
    name: d.name,
    provider: d.provider,
    action_type: d.action_type,
    payload_template: payload,
    on_failure: d.on_failure || undefined,
    delay_seconds: d.delay_seconds ? Number(d.delay_seconds) : undefined,
    branches: d.branches,
    default_next: d.default_next || undefined,
  }
}

function configToDraft(c: ChainStepConfig): StepDraft {
  if (c.sub_chain) {
    return { ...newStepDraft('sub_chain'), name: c.name, sub_chain: c.sub_chain }
  }
  if (c.parallel) {
    const p = c.parallel
    return {
      ...newStepDraft('parallel'),
      name: c.name,
      parallel_join: p.join ?? 'All',
      parallel_failure: p.on_failure ?? 'FailFast',
      parallel_timeout: p.timeout_seconds?.toString() ?? '',
      parallel_concurrency: p.max_concurrency?.toString() ?? '',
      parallel_sub_steps: (p.steps ?? []).map((s: ParallelSubStepConfig) => ({
        name: s.name,
        provider: s.provider,
        action_type: s.action_type,
        payload_template_json: JSON.stringify(s.payload_template ?? {}, null, 2),
        on_failure: s.on_failure ?? '',
      })),
    }
  }
  return {
    ...newStepDraft('provider'),
    name: c.name,
    provider: c.provider,
    action_type: c.action_type,
    payload_template_json: JSON.stringify(c.payload_template ?? {}, null, 2),
    on_failure: c.on_failure ?? '',
    delay_seconds: c.delay_seconds?.toString() ?? '',
    branches: c.branches ?? [],
    default_next: c.default_next ?? '',
  }
}

function definitionToJson(def: ChainDefinition): string {
  return JSON.stringify(def, null, 2)
}

function stepTypeBadge(step: ChainStepConfig) {
  if (step.sub_chain) return <span className={styles.badgeSubChain}>Sub-Chain</span>
  if (step.parallel) return <span className={styles.badgeParallel}>Parallel</span>
  return <span className={styles.badgeProvider}>Provider</span>
}

// ---- Column definition ----

const col = createColumnHelper<ChainDefinitionSummary>()

// ---- Main Component ----

export function ChainDefinitions() {
  const { toast } = useToast()

  const [showCreate, setShowCreate] = useState(false)
  const [editTarget, setEditTarget] = useState<ChainDefinition | null>(null)
  const [selectedName, setSelectedName] = useState<string | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null)
  const [drawerTab, setDrawerTab] = useState('overview')

  const { data, isLoading } = useChainDefinitions()
  const { data: selectedDef } = useChainDefinition(selectedName ?? undefined)
  const saveMutation = useSaveChainDefinition()
  const deleteMutation = useDeleteChainDefinition()

  const handleDelete = () => {
    if (!deleteTarget) return
    deleteMutation.mutate(deleteTarget, {
      onSuccess: () => {
        toast('success', 'Definition deleted')
        setDeleteTarget(null)
        if (selectedName === deleteTarget) setSelectedName(null)
      },
      onError: (e) => toast('error', 'Delete failed', (e as Error).message),
    })
  }

  const handleSave = (def: ChainDefinition) => {
    saveMutation.mutate(def, {
      onSuccess: () => {
        toast('success', editTarget ? 'Definition updated' : 'Definition created')
        setShowCreate(false)
        setEditTarget(null)
      },
      onError: (e) => toast('error', 'Save failed', (e as Error).message),
    })
  }

  const columns = [
    col.accessor('name', {
      header: 'Name',
      cell: (info) => <span className={styles.nameCell}>{info.getValue()}</span>,
    }),
    col.accessor('steps_count', {
      header: 'Steps',
      cell: (info) => <span className={shared.detailValue}>{info.getValue()}</span>,
    }),
    col.display({
      id: 'features',
      header: 'Features',
      cell: (info) => {
        const row = info.row.original
        return (
          <div className={styles.featureBadges}>
            {row.has_branches && <Badge variant="info">Branches</Badge>}
            {row.has_parallel && <Badge variant="warning">Parallel</Badge>}
            {row.has_sub_chains && <Badge variant="neutral">Sub-Chains</Badge>}
          </div>
        )
      },
    }),
    col.accessor('on_failure', {
      header: 'Failure Policy',
      cell: (info) => <Badge variant="neutral">{info.getValue()}</Badge>,
    }),
    col.accessor('timeout_seconds', {
      header: 'Timeout',
      cell: (info) => {
        const val = info.getValue()
        return val ? <span className={shared.detailValue}>{val}s</span> : <span className={shared.detailValue}>-</span>
      },
    }),
    col.display({
      id: 'actions',
      header: 'Actions',
      cell: (info) => {
        const row = info.row.original
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
              icon={<Trash2 className="h-3.5 w-3.5" />}
              onClick={() => setDeleteTarget(row.name)}
              aria-label="Delete"
            >
              Delete
            </Button>
          </div>
        )
      },
    }),
  ]

  return (
    <div>
      <PageHeader
        title="Chain Definitions"
        subtitle="Manage reusable chain workflow definitions"
        actions={
          <div className={styles.headerActions}>
            <Button
              icon={<Plus className="h-3.5 w-3.5" />}
              onClick={() => setShowCreate(true)}
            >
              Create Definition
            </Button>
          </div>
        }
      />

      <DataTable
        data={data?.definitions ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={(row) => {
          setSelectedName(row.name)
          setDrawerTab('overview')
        }}
        emptyTitle="No chain definitions"
        emptyDescription="Create a chain definition to build reusable multi-step workflows."
      />

      {/* Create modal */}
      <ChainDefinitionModal
        open={showCreate}
        onClose={() => setShowCreate(false)}
        onSave={handleSave}
        loading={saveMutation.isPending}
        initial={null}
      />

      {/* Edit modal */}
      {editTarget && (
        <ChainDefinitionModal
          open={!!editTarget}
          onClose={() => setEditTarget(null)}
          onSave={handleSave}
          loading={saveMutation.isPending}
          initial={editTarget}
        />
      )}

      {/* Detail drawer */}
      <Drawer
        open={!!selectedName}
        onClose={() => setSelectedName(null)}
        title={selectedName ?? ''}
        wide
      >
        {selectedName && (
          <DefinitionDetailView
            name={selectedName}
            definition={selectedDef ?? null}
            tab={drawerTab}
            onTabChange={setDrawerTab}
            onEdit={() => { if (selectedDef) setEditTarget(selectedDef) }}
            onDelete={() => setDeleteTarget(selectedName)}
          />
        )}
      </Drawer>

      {/* Delete confirmation */}
      <DeleteConfirmModal
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={handleDelete}
        loading={deleteMutation.isPending}
        title="Delete Chain Definition"
        name={deleteTarget ?? ''}
        warning="This will permanently remove the chain definition. Running chains using this definition will not be affected."
      />
    </div>
  )
}

// ---- Detail View ----

function DefinitionDetailView({ name, definition, tab, onTabChange, onEdit, onDelete }: {
  name: string
  definition: ChainDefinition | null
  tab: string
  onTabChange: (t: string) => void
  onEdit: () => void
  onDelete: () => void
}) {
  const { data: dag } = useChainDefinitionDag(name)

  return (
    <div>
      <div className={styles.tabContainer}>
        <Tabs
          tabs={[
            { id: 'overview', label: 'Overview' },
            { id: 'steps', label: 'Steps' },
            { id: 'dag', label: 'DAG' },
          ]}
          active={tab}
          onChange={onTabChange}
          size="sm"
        />
      </div>

      {tab === 'overview' && definition && (
        <div className={styles.detailContent}>
          {([
            ['Name', definition.name],
            ['Failure Policy', definition.on_failure],
            ['Timeout', definition.timeout_seconds ? `${definition.timeout_seconds}s` : '-'],
            ['Steps', String(definition.steps.length)],
            ['Cancel Notification Provider', definition.on_cancel?.provider ?? '-'],
            ['Cancel Notification Action', definition.on_cancel?.action_type ?? '-'],
          ] as [string, string][]).map(([k, v]) => (
            <div key={k} className={shared.detailRow}>
              <span className={shared.detailLabel}>{k}</span>
              <span className={styles.detailValueWrap}>{v}</span>
            </div>
          ))}

          <div className={shared.actionButtons}>
            <Button
              variant="secondary"
              size="sm"
              icon={<Pencil className="h-3.5 w-3.5" />}
              onClick={onEdit}
            >
              Edit
            </Button>
            <Button
              variant="danger"
              size="sm"
              icon={<Trash2 className="h-3.5 w-3.5" />}
              onClick={onDelete}
            >
              Delete
            </Button>
          </div>
        </div>
      )}

      {tab === 'steps' && definition && (
        <div>
          <p className={shared.detailLabel}>{definition.steps.length} step(s)</p>
          <div className={styles.stepList}>
            {definition.steps.map((step) => (
              <ReadOnlyStepCard key={step.name} step={step} />
            ))}
          </div>
        </div>
      )}

      {tab === 'dag' && (
        <div>
          {dag ? (
            <div className={styles.dagContainer}>
              <DagPreview dag={dag} />
            </div>
          ) : (
            <p className={shared.detailLabel}>No DAG data available.</p>
          )}
        </div>
      )}
    </div>
  )
}

// ---- Lazy DAG preview (avoids circular deps by dynamic import) ----

import { ChainDAG } from '../components/dag/ChainDAG'
import type { DagResponse } from '../types'

function DagPreview({ dag }: { dag: DagResponse }) {
  // ChainDAG requires a ChainDetailResponse for the fallback path but we pass a dag directly
  const fakeChain = {
    chain_id: '',
    chain_name: dag.chain_name,
    status: dag.status ?? 'completed',
    current_step: 0,
    total_steps: dag.nodes.length,
    steps: [],
    started_at: '',
    updated_at: '',
    execution_path: [],
  }
  return <ChainDAG chain={fakeChain} dag={dag} />
}

// ---- Read-only step card ----

function ReadOnlyStepCard({ step }: { step: ChainStepConfig }) {
  const [open, setOpen] = useState(false)
  return (
    <div className={styles.stepCard}>
      <button
        type="button"
        className={styles.stepCardHeader}
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        {stepTypeBadge(step)}
        <span className={styles.stepName}>{step.name}</span>
        <ChevronDown className={`${styles.stepChevron} ${open ? styles.stepChevronOpen : ''}`} aria-hidden />
      </button>
      {open && (
        <div className={styles.stepCardBody}>
          {step.provider && (
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Provider</span>
              <span className={shared.detailValue}>{step.provider}</span>
            </div>
          )}
          {step.action_type && (
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Action Type</span>
              <span className={shared.detailValue}>{step.action_type}</span>
            </div>
          )}
          {step.sub_chain && (
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>Sub-Chain</span>
              <span className={shared.detailValue}>{step.sub_chain}</span>
            </div>
          )}
          {step.on_failure && (
            <div className={shared.detailRow}>
              <span className={shared.detailLabel}>On Failure</span>
              <span className={shared.detailValue}>{step.on_failure}</span>
            </div>
          )}
        </div>
      )}
    </div>
  )
}

// ---- Chain Definition Modal ----

const ON_FAILURE_OPTIONS = [
  { value: 'Abort', label: 'Abort' },
  { value: 'AbortNoDlq', label: 'Abort (no DLQ)' },
]

const STEP_FAILURE_OPTIONS = [
  { value: '', label: '(default)' },
  { value: 'Abort', label: 'Abort' },
  { value: 'Skip', label: 'Skip' },
  { value: 'Dlq', label: 'DLQ' },
]

const STEP_TYPE_OPTIONS = [
  { value: 'provider', label: 'Provider' },
  { value: 'sub_chain', label: 'Sub-Chain' },
  { value: 'parallel', label: 'Parallel' },
]

const JOIN_OPTIONS = [
  { value: 'All', label: 'All (all must succeed)' },
  { value: 'Any', label: 'Any (first success wins)' },
]

const PARALLEL_FAIL_OPTIONS = [
  { value: 'FailFast', label: 'FailFast (cancel on first failure)' },
  { value: 'BestEffort', label: 'BestEffort (run all)' },
]

const BRANCH_OP_OPTIONS = [
  { value: 'Eq', label: 'Eq' },
  { value: 'Neq', label: 'Neq' },
  { value: 'Contains', label: 'Contains' },
  { value: 'Exists', label: 'Exists' },
]

function ChainDefinitionModal({ open, onClose, onSave, loading, initial }: {
  open: boolean
  onClose: () => void
  onSave: (def: ChainDefinition) => void
  loading: boolean
  initial: ChainDefinition | null
}) {
  const isEdit = !!initial

  // Metadata
  const [name, setName] = useState(initial?.name ?? '')
  const [onFailure, setOnFailure] = useState(initial?.on_failure ?? 'Abort')
  const [timeout, setTimeout] = useState(initial?.timeout_seconds?.toString() ?? '')
  const [showCancelNotif, setShowCancelNotif] = useState(!!initial?.on_cancel)
  const [cancelProvider, setCancelProvider] = useState(initial?.on_cancel?.provider ?? '')
  const [cancelAction, setCancelAction] = useState(initial?.on_cancel?.action_type ?? '')

  // Steps
  const [steps, setSteps] = useState<StepDraft[]>(() =>
    (initial?.steps ?? []).map(configToDraft),
  )
  const [expandedSteps, setExpandedSteps] = useState<Set<number>>(new Set())
  const [newStepType, setNewStepType] = useState<StepType>('provider')

  // Tab: 'visual' | 'json'
  const [activeTab, setActiveTab] = useState('visual')
  const [jsonText, setJsonText] = useState(() =>
    definitionToJson(initial ?? { name: '', steps: [], on_failure: 'Abort' }),
  )
  const [jsonError, setJsonError] = useState<string | null>(null)

  // Sync visual -> JSON whenever visual state changes
  function buildDefinition(): ChainDefinition {
    return {
      name,
      on_failure: onFailure,
      timeout_seconds: timeout ? Number(timeout) : undefined,
      on_cancel: showCancelNotif && cancelProvider
        ? { provider: cancelProvider, action_type: cancelAction }
        : undefined,
      steps: steps.map(draftToConfig),
    }
  }

  function syncVisualToJson() {
    setJsonText(definitionToJson(buildDefinition()))
    setJsonError(null)
  }

  function syncJsonToVisual(text: string) {
    setJsonText(text)
    try {
      const parsed = JSON.parse(text) as ChainDefinition
      setName(parsed.name ?? '')
      setOnFailure(parsed.on_failure ?? 'Abort')
      setTimeout(parsed.timeout_seconds?.toString() ?? '')
      const hasCancel = !!(parsed.on_cancel?.provider)
      setShowCancelNotif(hasCancel)
      setCancelProvider(parsed.on_cancel?.provider ?? '')
      setCancelAction(parsed.on_cancel?.action_type ?? '')
      setSteps((parsed.steps ?? []).map(configToDraft))
      setJsonError(null)
    } catch (e) {
      setJsonError((e as Error).message)
    }
  }

  function handleTabChange(tab: string) {
    if (tab === 'json' && activeTab === 'visual') {
      syncVisualToJson()
    }
    setActiveTab(tab)
  }

  // Step helpers
  function toggleStep(idx: number) {
    setExpandedSteps((prev) => {
      const next = new Set(prev)
      if (next.has(idx)) next.delete(idx)
      else next.add(idx)
      return next
    })
  }

  function updateStep(idx: number, patch: Partial<StepDraft>) {
    setSteps((prev) => prev.map((s, i) => i === idx ? { ...s, ...patch } : s))
  }

  function removeStep(idx: number) {
    setSteps((prev) => prev.filter((_, i) => i !== idx))
    setExpandedSteps((prev) => {
      const next = new Set<number>()
      prev.forEach((v) => { if (v < idx) next.add(v); else if (v > idx) next.add(v - 1) })
      return next
    })
  }

  function moveStep(idx: number, dir: -1 | 1) {
    const target = idx + dir
    if (target < 0 || target >= steps.length) return
    setSteps((prev) => {
      const next = [...prev]
      ;[next[idx], next[target]] = [next[target], next[idx]]
      return next
    })
    setExpandedSteps((prev) => {
      const next = new Set<number>()
      prev.forEach((v) => {
        if (v === idx) next.add(target)
        else if (v === target) next.add(idx)
        else next.add(v)
      })
      return next
    })
  }

  function addStep() {
    const draft = newStepDraft(newStepType)
    setSteps((prev) => [...prev, draft])
    setExpandedSteps((prev) => new Set([...prev, steps.length]))
  }

  // Branch helpers for a step
  function addBranch(stepIdx: number) {
    updateStep(stepIdx, {
      branches: [...steps[stepIdx].branches, { field: '', operator: 'Eq', value: '', target: '' }],
    })
  }

  function updateBranch(stepIdx: number, branchIdx: number, patch: Partial<BranchCondition>) {
    const next = steps[stepIdx].branches.map((b, i) => i === branchIdx ? { ...b, ...patch } : b)
    updateStep(stepIdx, { branches: next })
  }

  function removeBranch(stepIdx: number, branchIdx: number) {
    updateStep(stepIdx, { branches: steps[stepIdx].branches.filter((_, i) => i !== branchIdx) })
  }

  // Parallel sub-step helpers
  function addSubStep(stepIdx: number) {
    updateStep(stepIdx, { parallel_sub_steps: [...steps[stepIdx].parallel_sub_steps, newSubStep()] })
  }

  function updateSubStep(stepIdx: number, subIdx: number, patch: Partial<ParallelSubStep>) {
    const next = steps[stepIdx].parallel_sub_steps.map((s, i) => i === subIdx ? { ...s, ...patch } : s)
    updateStep(stepIdx, { parallel_sub_steps: next })
  }

  function removeSubStep(stepIdx: number, subIdx: number) {
    updateStep(stepIdx, {
      parallel_sub_steps: steps[stepIdx].parallel_sub_steps.filter((_, i) => i !== subIdx),
    })
  }

  function handleSave() {
    if (activeTab === 'json') {
      try {
        const parsed = JSON.parse(jsonText) as ChainDefinition
        onSave(parsed)
      } catch (e) {
        setJsonError((e as Error).message)
      }
    } else {
      onSave(buildDefinition())
    }
  }

  const isValid = name.trim().length > 0

  return (
    <Modal
      key={initial?.name ?? 'create'}
      open={open}
      onClose={onClose}
      title={isEdit ? `Edit — ${initial?.name}` : 'Create Chain Definition'}
      size="xl"
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>Cancel</Button>
          <Button
            loading={loading}
            onClick={handleSave}
            disabled={!isValid}
            icon={<GitBranch className="h-3.5 w-3.5" />}
          >
            {isEdit ? 'Update' : 'Create'}
          </Button>
        </>
      }
    >
      <div className={styles.modalTabBar}>
        <Tabs
          tabs={[
            { id: 'visual', label: 'Visual' },
            { id: 'json', label: 'JSON' },
          ]}
          active={activeTab}
          onChange={handleTabChange}
          size="sm"
        />
      </div>

      {activeTab === 'visual' && (
        <div className={shared.formSection}>
          {/* Metadata */}
          <div className={shared.formGrid}>
            <Input
              label="Name *"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="my-chain"
              disabled={isEdit}
            />
            <Select
              label="Failure Policy *"
              options={ON_FAILURE_OPTIONS}
              value={onFailure}
              onChange={(e) => setOnFailure(e.target.value)}
            />
          </div>

          <div className={shared.formGrid}>
            <Input
              label="Timeout (seconds)"
              type="number"
              value={timeout}
              onChange={(e) => setTimeout(e.target.value)}
              placeholder="(none)"
              min="1"
            />
            <div />
          </div>

          {/* Cancel notification */}
          <div>
            <button
              type="button"
              className={styles.collapsibleToggle}
              onClick={() => setShowCancelNotif((v) => !v)}
              aria-expanded={showCancelNotif}
            >
              <ChevronDown
                className={`${styles.stepChevron} ${showCancelNotif ? styles.stepChevronOpen : ''}`}
                aria-hidden
              />
              Cancel Notification
            </button>
            {showCancelNotif && (
              <div className={styles.cancelNotifSection}>
                <div className={shared.formGrid}>
                  <Input
                    label="Provider"
                    value={cancelProvider}
                    onChange={(e) => setCancelProvider(e.target.value)}
                    placeholder="slack"
                  />
                  <Input
                    label="Action Type"
                    value={cancelAction}
                    onChange={(e) => setCancelAction(e.target.value)}
                    placeholder="send-message"
                  />
                </div>
              </div>
            )}
          </div>

          {/* Steps */}
          <div>
            <h3 className={shared.sectionTitle}>Steps ({steps.length})</h3>
            <div className={styles.stepList}>
              {steps.map((step, idx) => (
                <StepCard
                  key={idx}
                  step={step}
                  idx={idx}
                  total={steps.length}
                  expanded={expandedSteps.has(idx)}
                  onToggle={() => toggleStep(idx)}
                  onUpdate={(patch) => updateStep(idx, patch)}
                  onRemove={() => removeStep(idx)}
                  onMoveUp={() => moveStep(idx, -1)}
                  onMoveDown={() => moveStep(idx, 1)}
                  onAddBranch={() => addBranch(idx)}
                  onUpdateBranch={(bi, patch) => updateBranch(idx, bi, patch)}
                  onRemoveBranch={(bi) => removeBranch(idx, bi)}
                  onAddSubStep={() => addSubStep(idx)}
                  onUpdateSubStep={(si, patch) => updateSubStep(idx, si, patch)}
                  onRemoveSubStep={(si) => removeSubStep(idx, si)}
                />
              ))}
            </div>

            <div className={styles.addStepBar}>
              <Select
                options={STEP_TYPE_OPTIONS}
                value={newStepType}
                onChange={(e) => setNewStepType(e.target.value as StepType)}
              />
              <Button
                size="sm"
                variant="secondary"
                icon={<Plus className="h-3.5 w-3.5" />}
                onClick={addStep}
              >
                Add Step
              </Button>
            </div>
          </div>
        </div>
      )}

      {activeTab === 'json' && (
        <div>
          <label className={shared.textareaLabel} htmlFor="chain-def-json">
            Chain Definition JSON
          </label>
          <textarea
            id="chain-def-json"
            className={styles.jsonTextarea}
            value={jsonText}
            onChange={(e) => syncJsonToVisual(e.target.value)}
            spellCheck={false}
            aria-describedby={jsonError ? 'json-error' : undefined}
          />
          {jsonError && (
            <p id="json-error" role="alert" className={styles.jsonError}>
              {jsonError}
            </p>
          )}
        </div>
      )}
    </Modal>
  )
}

// ---- Step Card (editable) ----

interface StepCardProps {
  step: StepDraft
  idx: number
  total: number
  expanded: boolean
  onToggle: () => void
  onUpdate: (patch: Partial<StepDraft>) => void
  onRemove: () => void
  onMoveUp: () => void
  onMoveDown: () => void
  onAddBranch: () => void
  onUpdateBranch: (bi: number, patch: Partial<BranchCondition>) => void
  onRemoveBranch: (bi: number) => void
  onAddSubStep: () => void
  onUpdateSubStep: (si: number, patch: Partial<ParallelSubStep>) => void
  onRemoveSubStep: (si: number) => void
}

function StepCard({
  step, idx, total, expanded, onToggle, onUpdate, onRemove,
  onMoveUp, onMoveDown, onAddBranch, onUpdateBranch, onRemoveBranch,
  onAddSubStep, onUpdateSubStep, onRemoveSubStep,
}: StepCardProps) {
  const [showBranches, setShowBranches] = useState(step.branches.length > 0)

  const typeBadge = step.type === 'sub_chain'
    ? <span className={styles.badgeSubChain}>Sub-Chain</span>
    : step.type === 'parallel'
      ? <span className={styles.badgeParallel}>Parallel</span>
      : <span className={styles.badgeProvider}>Provider</span>

  return (
    <div className={styles.stepCard}>
      <div className={styles.stepCardHeader}>
        {typeBadge}
        <button
          type="button"
          className={styles.stepName}
          onClick={onToggle}
          aria-expanded={expanded}
          aria-label={`${expanded ? 'Collapse' : 'Expand'} step ${step.name || idx + 1}`}
        >
          {step.name || `(step ${idx + 1})`}
        </button>
        <div className={styles.stepReorderGroup}>
          <button
            type="button"
            className={styles.reorderBtn}
            onClick={onMoveUp}
            disabled={idx === 0}
            aria-label="Move step up"
          >
            <ArrowUp className="h-3.5 w-3.5" aria-hidden />
          </button>
          <button
            type="button"
            className={styles.reorderBtn}
            onClick={onMoveDown}
            disabled={idx === total - 1}
            aria-label="Move step down"
          >
            <ArrowDown className="h-3.5 w-3.5" aria-hidden />
          </button>
          <Button
            variant="ghost"
            size="sm"
            icon={<Trash2 className="h-3.5 w-3.5" />}
            onClick={onRemove}
            aria-label="Remove step"
          />
        </div>
        <ChevronDown
          className={`${styles.stepChevron} ${expanded ? styles.stepChevronOpen : ''}`}
          aria-hidden
          onClick={onToggle}
        />
      </div>

      {expanded && (
        <div className={styles.stepCardBody}>
          <Input
            label="Step Name *"
            value={step.name}
            onChange={(e) => onUpdate({ name: e.target.value })}
            placeholder="send-notification"
          />

          {step.type === 'provider' && (
            <ProviderStepFields
              step={step}
              onUpdate={onUpdate}
              showBranches={showBranches}
              onToggleBranches={() => setShowBranches((v) => !v)}
              onAddBranch={onAddBranch}
              onUpdateBranch={onUpdateBranch}
              onRemoveBranch={onRemoveBranch}
            />
          )}

          {step.type === 'sub_chain' && (
            <Input
              label="Sub-Chain Name *"
              value={step.sub_chain}
              onChange={(e) => onUpdate({ sub_chain: e.target.value })}
              placeholder="notification-chain"
            />
          )}

          {step.type === 'parallel' && (
            <ParallelStepFields
              step={step}
              onUpdate={onUpdate}
              onAddSubStep={onAddSubStep}
              onUpdateSubStep={onUpdateSubStep}
              onRemoveSubStep={onRemoveSubStep}
            />
          )}
        </div>
      )}
    </div>
  )
}

// ---- Provider Step Fields ----

function ProviderStepFields({ step, onUpdate, showBranches, onToggleBranches, onAddBranch, onUpdateBranch, onRemoveBranch }: {
  step: StepDraft
  onUpdate: (patch: Partial<StepDraft>) => void
  showBranches: boolean
  onToggleBranches: () => void
  onAddBranch: () => void
  onUpdateBranch: (bi: number, patch: Partial<BranchCondition>) => void
  onRemoveBranch: (bi: number) => void
}) {
  return (
    <>
      <div className={shared.formGrid}>
        <Input
          label="Provider *"
          value={step.provider}
          onChange={(e) => onUpdate({ provider: e.target.value })}
          placeholder="slack"
        />
        <Input
          label="Action Type *"
          value={step.action_type}
          onChange={(e) => onUpdate({ action_type: e.target.value })}
          placeholder="send-message"
        />
      </div>

      <div>
        <label className={shared.textareaLabel} htmlFor={`payload-${step.name}`}>Payload Template (JSON)</label>
        <textarea
          id={`payload-${step.name}`}
          className={shared.textarea}
          value={step.payload_template_json}
          onChange={(e) => onUpdate({ payload_template_json: e.target.value })}
          rows={3}
          spellCheck={false}
        />
      </div>

      <div className={shared.formGrid}>
        <Select
          label="On Failure"
          options={STEP_FAILURE_OPTIONS}
          value={step.on_failure}
          onChange={(e) => onUpdate({ on_failure: e.target.value })}
        />
        <Input
          label="Delay (seconds)"
          type="number"
          value={step.delay_seconds}
          onChange={(e) => onUpdate({ delay_seconds: e.target.value })}
          placeholder="(none)"
          min="0"
        />
      </div>

      <Input
        label="Default Next Step"
        value={step.default_next}
        onChange={(e) => onUpdate({ default_next: e.target.value })}
        placeholder="(none)"
      />

      {/* Branches */}
      <div>
        <div className={styles.branchSectionHeader}>
          <button
            type="button"
            className={styles.collapsibleToggle}
            onClick={onToggleBranches}
            aria-expanded={showBranches}
          >
            <ChevronDown
              className={`${styles.stepChevron} ${showBranches ? styles.stepChevronOpen : ''}`}
              aria-hidden
            />
            Branches ({step.branches.length})
          </button>
          {showBranches && (
            <Button
              size="sm"
              variant="secondary"
              icon={<Plus className="h-3.5 w-3.5" />}
              onClick={onAddBranch}
            >
              Add Branch
            </Button>
          )}
        </div>
        {showBranches && step.branches.length > 0 && (
          <div className={styles.branchList}>
            {step.branches.map((branch, bi) => (
              <div key={bi} className={styles.branchRow}>
                <Input
                  placeholder="field"
                  value={branch.field}
                  onChange={(e) => onUpdateBranch(bi, { field: e.target.value })}
                  aria-label={`Branch ${bi + 1} field`}
                />
                <Select
                  options={BRANCH_OP_OPTIONS}
                  value={branch.operator}
                  onChange={(e) => onUpdateBranch(bi, { operator: e.target.value as BranchCondition['operator'] })}
                  aria-label={`Branch ${bi + 1} operator`}
                />
                <Input
                  placeholder="value"
                  value={String(branch.value ?? '')}
                  onChange={(e) => onUpdateBranch(bi, { value: e.target.value })}
                  aria-label={`Branch ${bi + 1} value`}
                />
                <Input
                  placeholder="target step"
                  value={branch.target}
                  onChange={(e) => onUpdateBranch(bi, { target: e.target.value })}
                  aria-label={`Branch ${bi + 1} target`}
                />
                <Button
                  variant="ghost"
                  size="sm"
                  icon={<Trash2 className="h-3.5 w-3.5" />}
                  onClick={() => onRemoveBranch(bi)}
                  aria-label={`Remove branch ${bi + 1}`}
                />
              </div>
            ))}
          </div>
        )}
      </div>
    </>
  )
}

// ---- Parallel Step Fields ----

function ParallelStepFields({ step, onUpdate, onAddSubStep, onUpdateSubStep, onRemoveSubStep }: {
  step: StepDraft
  onUpdate: (patch: Partial<StepDraft>) => void
  onAddSubStep: () => void
  onUpdateSubStep: (si: number, patch: Partial<ParallelSubStep>) => void
  onRemoveSubStep: (si: number) => void
}) {
  return (
    <>
      <div className={shared.formGrid}>
        <Select
          label="Join Policy"
          options={JOIN_OPTIONS}
          value={step.parallel_join}
          onChange={(e) => onUpdate({ parallel_join: e.target.value })}
        />
        <Select
          label="Failure Policy"
          options={PARALLEL_FAIL_OPTIONS}
          value={step.parallel_failure}
          onChange={(e) => onUpdate({ parallel_failure: e.target.value })}
        />
      </div>
      <div className={shared.formGrid}>
        <Input
          label="Timeout (seconds)"
          type="number"
          value={step.parallel_timeout}
          onChange={(e) => onUpdate({ parallel_timeout: e.target.value })}
          placeholder="(none)"
          min="1"
        />
        <Input
          label="Max Concurrency"
          type="number"
          value={step.parallel_concurrency}
          onChange={(e) => onUpdate({ parallel_concurrency: e.target.value })}
          placeholder="(unlimited)"
          min="1"
        />
      </div>

      <div>
        <div className={styles.branchSectionHeader}>
          <span className={shared.sectionTitle}>Sub-steps ({step.parallel_sub_steps.length})</span>
          <Button
            size="sm"
            variant="secondary"
            icon={<Plus className="h-3.5 w-3.5" />}
            onClick={onAddSubStep}
          >
            Add Sub-step
          </Button>
        </div>
        <div className={styles.subStepList}>
          {step.parallel_sub_steps.map((sub, si) => (
            <ParallelSubStepCard
              key={si}
              sub={sub}
              si={si}
              onUpdate={(patch) => onUpdateSubStep(si, patch)}
              onRemove={() => onRemoveSubStep(si)}
            />
          ))}
        </div>
      </div>
    </>
  )
}

// ---- Parallel Sub-step Card ----

function ParallelSubStepCard({ sub, si, onUpdate, onRemove }: {
  sub: ParallelSubStep
  si: number
  onUpdate: (patch: Partial<ParallelSubStep>) => void
  onRemove: () => void
}) {
  const [open, setOpen] = useState(true)
  return (
    <div className={styles.stepCard}>
      <div className={styles.stepCardHeader}>
        <span className={styles.badgeProvider}>Provider</span>
        <button
          type="button"
          className={styles.stepName}
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
          aria-label={`${open ? 'Collapse' : 'Expand'} sub-step ${sub.name || si + 1}`}
        >
          {sub.name || `(sub-step ${si + 1})`}
        </button>
        <Button
          variant="ghost"
          size="sm"
          icon={<Trash2 className="h-3.5 w-3.5" />}
          onClick={onRemove}
          aria-label="Remove sub-step"
        />
        <ChevronDown
          className={`${styles.stepChevron} ${open ? styles.stepChevronOpen : ''}`}
          aria-hidden
          onClick={() => setOpen((v) => !v)}
        />
      </div>
      {open && (
        <div className={styles.stepCardBody}>
          <Input
            label="Sub-step Name *"
            value={sub.name}
            onChange={(e) => onUpdate({ name: e.target.value })}
            placeholder="step-a"
          />
          <div className={shared.formGrid}>
            <Input
              label="Provider *"
              value={sub.provider}
              onChange={(e) => onUpdate({ provider: e.target.value })}
              placeholder="email"
            />
            <Input
              label="Action Type *"
              value={sub.action_type}
              onChange={(e) => onUpdate({ action_type: e.target.value })}
              placeholder="send"
            />
          </div>
          <div>
            <label className={shared.textareaLabel} htmlFor={`sub-payload-${si}`}>Payload Template (JSON)</label>
            <textarea
              id={`sub-payload-${si}`}
              className={shared.textarea}
              value={sub.payload_template_json}
              onChange={(e) => onUpdate({ payload_template_json: e.target.value })}
              rows={2}
              spellCheck={false}
            />
          </div>
          <Select
            label="On Failure"
            options={STEP_FAILURE_OPTIONS}
            value={sub.on_failure}
            onChange={(e) => onUpdate({ on_failure: e.target.value })}
          />
        </div>
      )}
    </div>
  )
}
