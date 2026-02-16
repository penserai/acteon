import { useState } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import {
  Plus, Trash2, Play,
} from 'lucide-react'
import {
  useWasmPlugins,
  useWasmPlugin,
  useDeleteWasmPlugin,
  useRegisterWasmPlugin,
  useTestWasmPlugin,
} from '../api/hooks/useWasmPlugins'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Modal } from '../components/ui/Modal'
import { Drawer } from '../components/ui/Drawer'
import { Tabs } from '../components/ui/Tabs'
import { JsonViewer } from '../components/ui/JsonViewer'
import { useToast } from '../components/ui/useToast'
import { relativeTime } from '../lib/format'
import type { WasmPlugin, WasmTestResponse } from '../types'
import styles from './WasmPlugins.module.css'

function formatBytes(bytes: number): string {
  if (bytes >= 1_048_576) return `${(bytes / 1_048_576).toFixed(0)} MB`
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`
  return `${bytes} B`
}

function formatCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return n.toString()
}

// ---- Column definition ----

const col = createColumnHelper<WasmPlugin>()

// ---- Component ----

export function WasmPlugins() {
  const { toast } = useToast()

  // Register modal
  const [showRegister, setShowRegister] = useState(false)

  // Detail drawer
  const [selectedName, setSelectedName] = useState<string | null>(null)
  const [detailTab, setDetailTab] = useState('overview')

  // Delete confirmation
  const [deleteTarget, setDeleteTarget] = useState<WasmPlugin | null>(null)

  // Data
  const { data, isLoading } = useWasmPlugins()
  const { data: selectedPlugin } = useWasmPlugin(selectedName ?? undefined)

  // Mutations
  const registerMutation = useRegisterWasmPlugin()
  const deleteMutation = useDeleteWasmPlugin()

  const handleDelete = () => {
    if (!deleteTarget) return
    deleteMutation.mutate(deleteTarget.name, {
      onSuccess: () => {
        toast('success', 'Plugin deleted')
        setDeleteTarget(null)
        if (selectedName === deleteTarget.name) setSelectedName(null)
      },
      onError: (e) => toast('error', 'Delete failed', (e as Error).message),
    })
  }

  const columns = [
    col.accessor('name', {
      header: 'Name',
      cell: (info) => <span className={styles.nameCell}>{info.getValue()}</span>,
    }),
    col.accessor('description', {
      header: 'Description',
      cell: (info) => (
        <span className={styles.descCell}>{info.getValue() ?? '-'}</span>
      ),
    }),
    col.accessor('enabled', {
      header: 'Status',
      cell: (info) => (
        <Badge variant={info.getValue() ? 'success' : 'neutral'}>
          {info.getValue() ? 'Enabled' : 'Disabled'}
        </Badge>
      ),
    }),
    col.accessor('invocation_count', {
      header: 'Invocations',
      cell: (info) => (
        <span className={styles.monoCell}>{formatCount(info.getValue())}</span>
      ),
    }),
    col.accessor('last_invoked_at', {
      header: 'Last Used',
      cell: (info) => {
        const val = info.getValue()
        return val ? relativeTime(val) : 'Never'
      },
    }),
    col.accessor('memory_limit_bytes', {
      header: 'Memory',
      cell: (info) => (
        <span className={styles.monoCell}>{formatBytes(info.getValue())}</span>
      ),
    }),
    col.accessor('timeout_ms', {
      header: 'Timeout',
      cell: (info) => (
        <span className={styles.monoCell}>{info.getValue()}ms</span>
      ),
    }),
    col.display({
      id: 'actions',
      header: 'Actions',
      cell: (info) => {
        const row = info.row.original
        return (
          <div
            className={styles.actionsCell}
            onClick={(e) => e.stopPropagation()}
            role="group"
            aria-label="Row actions"
          >
            <Button
              variant="ghost"
              size="sm"
              icon={<Trash2 className="h-3.5 w-3.5" />}
              onClick={() => setDeleteTarget(row)}
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
        title="WASM Plugins"
        subtitle="Manage WebAssembly rule plugins"
        actions={
          <div className={styles.headerActions}>
            <Button
              icon={<Plus className="h-3.5 w-3.5" />}
              onClick={() => setShowRegister(true)}
            >
              Register Plugin
            </Button>
          </div>
        }
      />

      <DataTable
        data={data?.plugins ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={(row) => {
          setSelectedName(row.name)
          setDetailTab('overview')
        }}
        emptyTitle="No WASM plugins"
        emptyDescription="Register a .wasm plugin to extend rule evaluation with custom logic."
      />

      {/* Register modal */}
      <RegisterPluginModal
        open={showRegister}
        onClose={() => setShowRegister(false)}
        onSubmit={(formData) => {
          registerMutation.mutate(formData, {
            onSuccess: (res) => {
              toast('success', 'Plugin registered', `Name: ${res.name}`)
              setShowRegister(false)
            },
            onError: (e) => toast('error', 'Registration failed', (e as Error).message),
          })
        }}
        loading={registerMutation.isPending}
      />

      {/* Detail drawer */}
      <Drawer
        open={!!selectedName}
        onClose={() => setSelectedName(null)}
        title={selectedPlugin?.name ?? selectedName ?? ''}
        wide
      >
        {selectedPlugin && (
          <PluginDetailView
            plugin={selectedPlugin}
            tab={detailTab}
            onTabChange={setDetailTab}
            onDelete={() => setDeleteTarget(selectedPlugin)}
          />
        )}
      </Drawer>

      {/* Delete confirmation modal */}
      <Modal
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        title="Delete Plugin"
        size="sm"
        footer={
          <>
            <Button variant="secondary" onClick={() => setDeleteTarget(null)}>Cancel</Button>
            <Button
              variant="danger"
              loading={deleteMutation.isPending}
              onClick={handleDelete}
            >
              Delete
            </Button>
          </>
        }
      >
        <p className={styles.deleteWarning}>
          Are you sure you want to delete the plugin{' '}
          <span className={styles.deleteName}>{deleteTarget?.name}</span>?
          This will unload the WASM module and any rules referencing it will fail.
        </p>
      </Modal>
    </div>
  )
}

// ---- Register Plugin Modal ----

function RegisterPluginModal({ open, onClose, onSubmit, loading }: {
  open: boolean
  onClose: () => void
  onSubmit: (formData: FormData) => void
  loading: boolean
}) {
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [memoryLimit, setMemoryLimit] = useState('16777216')
  const [timeoutMs, setTimeoutMs] = useState('100')
  const [file, setFile] = useState<File | null>(null)

  const handleSubmit = () => {
    if (!file || !name) return
    const formData = new FormData()
    formData.append('name', name)
    formData.append('wasm_file', file)
    if (description) formData.append('description', description)
    formData.append('memory_limit_bytes', memoryLimit)
    formData.append('timeout_ms', timeoutMs)
    formData.append('enabled', 'true')
    onSubmit(formData)
  }

  const isValid = name && file

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Register WASM Plugin"
      size="lg"
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>Cancel</Button>
          <Button
            loading={loading}
            onClick={handleSubmit}
            disabled={!isValid}
            icon={<Plus className="h-3.5 w-3.5" />}
          >
            Register
          </Button>
        </>
      }
    >
      <div className={styles.formSection}>
        <Input
          label="Plugin Name *"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="my-plugin"
        />

        <div>
          <label className={styles.fileLabel} htmlFor="wasm-file">WASM File *</label>
          <input
            id="wasm-file"
            type="file"
            accept=".wasm"
            className={styles.fileInput}
            onChange={(e) => setFile(e.target.files?.[0] ?? null)}
          />
        </div>

        <Input
          label="Description"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="A brief description of what this plugin does"
        />

        <div className={styles.formGrid}>
          <Input
            label="Memory Limit (bytes)"
            type="number"
            value={memoryLimit}
            onChange={(e) => setMemoryLimit(e.target.value)}
            placeholder="16777216"
            min="1"
          />
          <Input
            label="Timeout (ms)"
            type="number"
            value={timeoutMs}
            onChange={(e) => setTimeoutMs(e.target.value)}
            placeholder="100"
            min="1"
          />
        </div>
      </div>
    </Modal>
  )
}

// ---- Detail View ----

function PluginDetailView({ plugin, tab, onTabChange, onDelete }: {
  plugin: WasmPlugin
  tab: string
  onTabChange: (t: string) => void
  onDelete: () => void
}) {
  return (
    <div>
      <Tabs
        tabs={[
          { id: 'overview', label: 'Overview' },
          { id: 'test', label: 'Test' },
        ]}
        active={tab}
        onChange={onTabChange}
        size="sm"
      />

      {tab === 'overview' && (
        <div className={styles.detailContent}>
          {Object.entries({
            'Name': plugin.name,
            'Description': plugin.description ?? '-',
            'Status': plugin.enabled ? 'Enabled' : 'Disabled',
            'Memory Limit': formatBytes(plugin.memory_limit_bytes),
            'Timeout': `${plugin.timeout_ms}ms`,
            'Invocations': plugin.invocation_count.toLocaleString(),
            'Last Invoked': plugin.last_invoked_at ? relativeTime(plugin.last_invoked_at) : 'Never',
            'Registered': relativeTime(plugin.registered_at),
          }).map(([k, v]) => (
            <div key={k} className={styles.detailRow}>
              <span className={styles.detailLabel}>{k}</span>
              <span className={styles.detailValueWrap}>{v}</span>
            </div>
          ))}

          <div className={styles.actionButtons}>
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

      {tab === 'test' && (
        <TestInvocationPanel pluginName={plugin.name} />
      )}
    </div>
  )
}

// ---- Test Invocation Panel ----

function TestInvocationPanel({ pluginName }: { pluginName: string }) {
  const testMutation = useTestWasmPlugin()
  const { toast } = useToast()

  const [functionName, setFunctionName] = useState('evaluate')
  const [inputJson, setInputJson] = useState(JSON.stringify({
    namespace: 'test',
    tenant: 'test',
    provider: 'test',
    action_type: 'test',
    payload: { key: 'value' },
    metadata: {},
  }, null, 2))
  const [result, setResult] = useState<WasmTestResponse | null>(null)

  const handleTest = () => {
    let parsed: Record<string, unknown>
    try {
      parsed = JSON.parse(inputJson)
    } catch (e) {
      toast('error', 'Invalid JSON', (e as Error).message)
      return
    }

    testMutation.mutate(
      { name: pluginName, body: { function: functionName, input: parsed } },
      {
        onSuccess: (res) => setResult(res),
        onError: (e) => toast('error', 'Test failed', (e as Error).message),
      },
    )
  }

  return (
    <div className={styles.testSection}>
      <h3 className={styles.testSectionTitle}>Test Invocation</h3>

      <Input
        label="Function Name"
        value={functionName}
        onChange={(e) => setFunctionName(e.target.value)}
        placeholder="evaluate"
      />

      <div style={{ marginTop: '0.75rem' }}>
        <label className={styles.textareaLabel}>Input (JSON)</label>
        <textarea
          value={inputJson}
          onChange={(e) => setInputJson(e.target.value)}
          className={styles.textarea}
        />
      </div>

      <div style={{ marginTop: '0.75rem' }}>
        <Button
          icon={<Play className="h-3.5 w-3.5" />}
          loading={testMutation.isPending}
          onClick={handleTest}
          size="sm"
        >
          Run Test
        </Button>
      </div>

      {result && (
        <div className={styles.testResultCard}>
          <div className={styles.testResultRow}>
            <span className={styles.detailLabel}>Verdict</span>
            <span className={result.verdict ? styles.verdictPass : styles.verdictFail}>
              {result.verdict ? 'PASS (true)' : 'FAIL (false)'}
            </span>
          </div>
          <div className={styles.testResultRow}>
            <span className={styles.detailLabel}>Duration</span>
            <span className={styles.detailValue}>{result.duration_us}us</span>
          </div>
          {result.message && (
            <div className={styles.testMessage}>
              Message: {result.message}
            </div>
          )}
          {result.metadata && Object.keys(result.metadata).length > 0 && (
            <div className={styles.testMetadata}>
              <JsonViewer data={result.metadata} />
            </div>
          )}
        </div>
      )}
    </div>
  )
}
