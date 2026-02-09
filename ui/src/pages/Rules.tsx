import { useState } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import { RefreshCw } from 'lucide-react'
import { useRules, useReloadRules, useToggleRule } from '../api/hooks/useRules'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Badge } from '../components/ui/Badge'
import { Button } from '../components/ui/Button'
import { Toggle } from '../components/ui/Toggle'
import { Input } from '../components/ui/Input'
import { Select } from '../components/ui/Select'
import { Drawer } from '../components/ui/Drawer'
import { JsonViewer } from '../components/ui/JsonViewer'
import { useToast } from '../components/ui/useToast'
import type { RuleSummary } from '../types'
import styles from './Rules.module.css'

const col = createColumnHelper<RuleSummary>()

export function Rules() {
  const { data: rules, isLoading } = useRules()
  const reload = useReloadRules()
  const toggleRule = useToggleRule()
  const { toast } = useToast()
  const [search, setSearch] = useState('')
  const [filterAction, setFilterAction] = useState('')
  const [filterEnabled, setFilterEnabled] = useState('')
  const [selected, setSelected] = useState<RuleSummary | null>(null)

  const handleReload = () => {
    reload.mutate(undefined, {
      onSuccess: () => toast('success', 'Rules reloaded'),
      onError: (e) => toast('error', 'Failed to reload rules', (e as Error).message),
    })
  }

  const handleToggle = (name: string, enabled: boolean) => {
    toggleRule.mutate({ name, enabled }, {
      onError: (e) => toast('error', 'Failed to toggle rule', (e as Error).message),
    })
  }

  const filtered = (rules ?? []).filter((r) => {
    if (search && !r.name.toLowerCase().includes(search.toLowerCase())) return false
    if (filterAction && r.action_type !== filterAction) return false
    if (filterEnabled === 'true' && !r.enabled) return false
    if (filterEnabled === 'false' && r.enabled) return false
    return true
  })

  const actionTypes = [...new Set((rules ?? []).map((r) => r.action_type))].sort()

  const columns = [
    col.accessor('priority', { header: 'Pri', cell: (info) => <span className={styles.priorityCell}>{info.getValue()}</span> }),
    col.accessor('name', { header: 'Name', cell: (info) => <span className={styles.ruleNameCell}>{info.getValue()}</span> }),
    col.accessor('description', { header: 'Description', cell: (info) => <span className={styles.descriptionCell}>{info.getValue() ?? '-'}</span> }),
    col.accessor('action_type', { header: 'Action', cell: (info) => <Badge>{info.getValue()}</Badge> }),
    col.accessor('enabled', {
      header: 'Enabled',
      cell: (info) => (
        <Toggle
          checked={info.getValue()}
          onChange={(v) => handleToggle(info.row.original.name, v)}
          label={`Toggle ${info.row.original.name}`}
        />
      ),
    }),
    col.accessor('source', { header: 'Source', cell: (info) => <span className={styles.sourceCell}>{info.getValue()}</span> }),
  ]

  return (
    <div>
      <PageHeader
        title="Rules"
        actions={
          <Button
            variant="secondary"
            size="sm"
            loading={reload.isPending}
            icon={<RefreshCw className="h-3.5 w-3.5" />}
            onClick={handleReload}
          >
            Reload Rules
          </Button>
        }
      />

      <div className={styles.filterBar}>
        <div className={styles.searchContainer}>
          <Input placeholder="Search rules..." value={search} onChange={(e) => setSearch(e.target.value)} />
        </div>
        <Select
          options={[{ value: '', label: 'All Actions' }, ...actionTypes.map((a) => ({ value: a, label: a }))]}
          value={filterAction}
          onChange={(e) => setFilterAction(e.target.value)}
        />
        <Select
          options={[
            { value: '', label: 'All' },
            { value: 'true', label: 'Enabled' },
            { value: 'false', label: 'Disabled' },
          ]}
          value={filterEnabled}
          onChange={(e) => setFilterEnabled(e.target.value)}
        />
      </div>

      <DataTable
        data={filtered}
        columns={columns}
        loading={isLoading}
        onRowClick={setSelected}
        emptyTitle="No rules loaded"
        emptyDescription="Add YAML rule files to your rules directory and click Reload, or create rules via the API."
      />

      <Drawer open={!!selected} onClose={() => setSelected(null)} title={selected?.name ?? 'Rule Detail'} wide>
        {selected && (
          <div className={styles.detailContent}>
            <div className={styles.metadataGrid}>
              <div><span className={styles.descriptionLabel}>Priority:</span> <span className="font-medium">{selected.priority}</span></div>
              <div><span className={styles.descriptionLabel}>Source:</span> <span className="font-medium">{selected.source}</span></div>
              <div><span className={styles.descriptionLabel}>Enabled:</span> <Badge>{selected.enabled ? 'Yes' : 'No'}</Badge></div>
              <div><span className={styles.descriptionLabel}>Action:</span> <Badge>{selected.action_type}</Badge></div>
            </div>
            {selected.description && (
              <div>
                <span className={styles.descriptionLabel}>Description:</span>
                <p className={styles.descriptionSection}>{selected.description}</p>
              </div>
            )}
            <div>
              <span className={styles.actionDetailsLabel}>Action Details:</span>
              <div className={styles.actionDetailsSection}>
                <JsonViewer data={selected.action_details} />
              </div>
            </div>
          </div>
        )}
      </Drawer>
    </div>
  )
}
