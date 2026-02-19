import { useState, useCallback } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import { Plus, Pencil, Trash2, FileText, Link, Play, AlertTriangle } from 'lucide-react'
import {
  useTemplates,
  useTemplate,
  useCreateTemplate,
  useUpdateTemplate,
  useDeleteTemplate,
  useTemplateProfiles,
  useTemplateProfile,
  useCreateProfile,
  useUpdateProfile,
  useDeleteProfile,
  useRenderPreview,
} from '../api/hooks/useTemplates'
import { PageHeader } from '../components/layout/PageHeader'
import { DataTable } from '../components/ui/DataTable'
import { Button } from '../components/ui/Button'
import { Input } from '../components/ui/Input'
import { Modal } from '../components/ui/Modal'
import { Drawer } from '../components/ui/Drawer'
import { Tabs } from '../components/ui/Tabs'
import { useToast } from '../components/ui/useToast'
import { relativeTime } from '../lib/format'
import type {
  Template,
  TemplateProfile,
  TemplateProfileField,
  CreateTemplateRequest,
  UpdateTemplateRequest,
  CreateProfileRequest,
  UpdateProfileRequest,
  RenderPreviewResponse,
} from '../types'
import styles from './Templates.module.css'

// ---- Top-level page ----

export function Templates() {
  const [activeTab, setActiveTab] = useState('templates')

  return (
    <div>
      <PageHeader
        title="Payload Templates"
        subtitle="Manage reusable payload templates and field profiles"
      />
      <div className={styles.topTabs}>
        <Tabs
          tabs={[
            { id: 'templates', label: 'Templates' },
            { id: 'profiles', label: 'Profiles' },
            { id: 'playground', label: 'Playground' },
          ]}
          active={activeTab}
          onChange={setActiveTab}
        />
      </div>

      {activeTab === 'templates' && <TemplatesTab />}
      {activeTab === 'profiles' && <ProfilesTab />}
      {activeTab === 'playground' && <PlaygroundTab />}
    </div>
  )
}

// ---- Templates Tab ----

const templateCol = createColumnHelper<Template>()

function TemplatesTab() {
  const { toast } = useToast()

  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')

  const [showCreate, setShowCreate] = useState(false)
  const [editTarget, setEditTarget] = useState<Template | null>(null)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<Template | null>(null)

  const { data, isLoading } = useTemplates({
    namespace: ns || undefined,
    tenant: tenant || undefined,
  })

  const { data: selectedTemplate } = useTemplate(selectedId ?? undefined)

  const createMutation = useCreateTemplate()
  const updateMutation = useUpdateTemplate()
  const deleteMutation = useDeleteTemplate()

  const handleDelete = () => {
    if (!deleteTarget) return
    deleteMutation.mutate(deleteTarget.id, {
      onSuccess: () => {
        toast('success', 'Template deleted')
        setDeleteTarget(null)
        if (selectedId === deleteTarget.id) setSelectedId(null)
      },
      onError: (e) => toast('error', 'Delete failed', (e as Error).message),
    })
  }

  const columns = [
    templateCol.accessor('name', {
      header: 'Name',
      cell: (info) => <span className={styles.nameCell}>{info.getValue()}</span>,
    }),
    templateCol.accessor('namespace', {
      header: 'Namespace',
      cell: (info) => <span className={styles.monoCell}>{info.getValue()}</span>,
    }),
    templateCol.accessor('tenant', {
      header: 'Tenant',
      cell: (info) => <span className={styles.monoCell}>{info.getValue()}</span>,
    }),
    templateCol.accessor('description', {
      header: 'Description',
      cell: (info) => (
        <span className={styles.descCell}>{info.getValue() ?? '-'}</span>
      ),
    }),
    templateCol.accessor('updated_at', {
      header: 'Updated',
      cell: (info) => relativeTime(info.getValue()),
    }),
    templateCol.display({
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
              icon={<Pencil className="h-3.5 w-3.5" />}
              onClick={() => setEditTarget(row)}
              aria-label="Edit template"
            >
              Edit
            </Button>
            <Button
              variant="ghost"
              size="sm"
              icon={<Trash2 className="h-3.5 w-3.5" />}
              onClick={() => setDeleteTarget(row)}
              aria-label="Delete template"
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
      <div className={styles.filterBar}>
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
        <Button
          icon={<Plus className="h-3.5 w-3.5" />}
          onClick={() => setShowCreate(true)}
        >
          Create Template
        </Button>
      </div>

      <DataTable
        data={data?.templates ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={(row) => setSelectedId(row.id)}
        emptyTitle="No templates"
        emptyDescription="Create a payload template to reuse structured content across actions."
      />

      {/* Create modal */}
      <TemplateFormModal
        key="create"
        open={showCreate}
        onClose={() => setShowCreate(false)}
        onSubmit={(req) => {
          createMutation.mutate(req, {
            onSuccess: (res) => {
              toast('success', 'Template created', `ID: ${res.id}`)
              setShowCreate(false)
            },
            onError: (e) => toast('error', 'Create failed', (e as Error).message),
          })
        }}
        loading={createMutation.isPending}
        title="Create Template"
      />

      {/* Edit modal */}
      {editTarget && (
        <TemplateFormModal
          key={editTarget.id}
          open={!!editTarget}
          onClose={() => setEditTarget(null)}
          onSubmit={(req) => {
            const body: UpdateTemplateRequest = {
              content: req.content,
              description: req.description,
              labels: req.labels,
            }
            updateMutation.mutate({ id: editTarget.id, body }, {
              onSuccess: () => {
                toast('success', 'Template updated')
                setEditTarget(null)
              },
              onError: (e) => toast('error', 'Update failed', (e as Error).message),
            })
          }}
          loading={updateMutation.isPending}
          title="Edit Template"
          initial={editTarget}
        />
      )}

      {/* Detail drawer */}
      <Drawer
        open={!!selectedId}
        onClose={() => setSelectedId(null)}
        title={selectedTemplate?.name ?? selectedId ?? ''}
        wide
      >
        {selectedTemplate && (
          <TemplateDetailView
            template={selectedTemplate}
            onEdit={() => {
              setEditTarget(selectedTemplate)
              setSelectedId(null)
            }}
            onDelete={() => setDeleteTarget(selectedTemplate)}
          />
        )}
      </Drawer>

      {/* Delete confirmation modal */}
      <Modal
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        title="Delete Template"
        size="sm"
        footer={
          <>
            <Button variant="secondary" onClick={() => setDeleteTarget(null)}>
              Cancel
            </Button>
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
          Are you sure you want to delete template{' '}
          <span className={styles.deleteName}>{deleteTarget?.name}</span>? Any
          profiles referencing this template will break.
        </p>
      </Modal>
    </div>
  )
}

// ---- Template Form Modal ----

function TemplateFormModal({
  open,
  onClose,
  onSubmit,
  loading,
  title,
  initial,
}: {
  open: boolean
  onClose: () => void
  onSubmit: (req: CreateTemplateRequest) => void
  loading: boolean
  title: string
  initial?: Template
}) {
  const isEdit = !!initial

  const [name, setName] = useState(initial?.name ?? '')
  const [ns, setNs] = useState(initial?.namespace ?? '')
  const [tenant, setTenant] = useState(initial?.tenant ?? '')
  const [description, setDescription] = useState(initial?.description ?? '')
  const [content, setContent] = useState(initial?.content ?? '')
  const [labelsText, setLabelsText] = useState(
    initial?.labels
      ? Object.entries(initial.labels)
          .map(([k, v]) => `${k}=${v}`)
          .join('\n')
      : '',
  )

  const handleSubmit = () => {
    const labels: Record<string, string> = {}
    for (const line of labelsText.split('\n')) {
      const trimmed = line.trim()
      if (!trimmed) continue
      const eqIdx = trimmed.indexOf('=')
      if (eqIdx > 0) {
        labels[trimmed.slice(0, eqIdx).trim()] = trimmed.slice(eqIdx + 1).trim()
      }
    }
    onSubmit({
      name,
      namespace: ns,
      tenant,
      content,
      description: description || undefined,
      labels: Object.keys(labels).length > 0 ? labels : undefined,
    })
  }

  const isValid = name && ns && tenant && content

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={title}
      size="lg"
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button
            loading={loading}
            onClick={handleSubmit}
            disabled={!isValid}
            icon={<FileText className="h-3.5 w-3.5" />}
          >
            {isEdit ? 'Update' : 'Create'}
          </Button>
        </>
      }
    >
      <div className={styles.formSection}>
        <div className={styles.formGrid}>
          <Input
            label="Name *"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="order-confirmation"
            disabled={isEdit}
          />
          <Input
            label="Description"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="Brief description"
          />
        </div>

        <div className={styles.formGrid}>
          <Input
            label="Namespace *"
            value={ns}
            onChange={(e) => setNs(e.target.value)}
            placeholder="prod"
            disabled={isEdit}
          />
          <Input
            label="Tenant *"
            value={tenant}
            onChange={(e) => setTenant(e.target.value)}
            placeholder="acme"
            disabled={isEdit}
          />
        </div>

        <div>
          <label className={styles.textareaLabel} htmlFor="template-content">
            Content *
          </label>
          <textarea
            id="template-content"
            className={styles.textareaContent}
            value={content}
            onChange={(e) => setContent(e.target.value)}
            placeholder={'{\n  "subject": "{{subject}}",\n  "body": "{{body}}"\n}'}
            aria-required="true"
          />
        </div>

        <div>
          <label className={styles.textareaLabel} htmlFor="template-labels">
            Labels (key=value, one per line)
          </label>
          <textarea
            id="template-labels"
            className={styles.textareaShort}
            value={labelsText}
            onChange={(e) => setLabelsText(e.target.value)}
            placeholder={"team=platform\nenv=prod"}
          />
        </div>
      </div>
    </Modal>
  )
}

// ---- Template Detail View ----

function TemplateDetailView({
  template,
  onEdit,
  onDelete,
}: {
  template: Template
  onEdit: () => void
  onDelete: () => void
}) {
  return (
    <div>
      <div className={styles.detailContent}>
        {(
          [
            ['ID', template.id],
            ['Name', template.name],
            ['Namespace', template.namespace],
            ['Tenant', template.tenant],
            ['Description', template.description ?? '-'],
            ['Created', relativeTime(template.created_at)],
            ['Updated', relativeTime(template.updated_at)],
          ] as [string, string][]
        ).map(([k, v]) => (
          <div key={k} className={styles.detailRow}>
            <span className={styles.detailLabel}>{k}</span>
            <span className={styles.detailValueWrap}>{v}</span>
          </div>
        ))}
      </div>

      {template.labels && Object.keys(template.labels).length > 0 && (
        <div>
          <h2 className={styles.sectionTitle}>Labels</h2>
          <div className={styles.detailContent}>
            {Object.entries(template.labels).map(([k, v]) => (
              <div key={k} className={styles.detailRow}>
                <span className={styles.detailLabel}>{k}</span>
                <span className={styles.detailValue}>{v}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      <h2 className={styles.sectionTitle}>Content</h2>
      <pre className={styles.contentPreview}>{template.content}</pre>

      <div className={styles.actionButtons}>
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
  )
}

// ---- Profiles Tab ----

const profileCol = createColumnHelper<TemplateProfile>()

function ProfilesTab() {
  const { toast } = useToast()

  const [ns, setNs] = useState('')
  const [tenant, setTenant] = useState('')

  const [showCreate, setShowCreate] = useState(false)
  const [editTarget, setEditTarget] = useState<TemplateProfile | null>(null)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<TemplateProfile | null>(null)

  const { data, isLoading } = useTemplateProfiles({
    namespace: ns || undefined,
    tenant: tenant || undefined,
  })

  const { data: selectedProfile } = useTemplateProfile(selectedId ?? undefined)

  const createMutation = useCreateProfile()
  const updateMutation = useUpdateProfile()
  const deleteMutation = useDeleteProfile()

  const handleDelete = () => {
    if (!deleteTarget) return
    deleteMutation.mutate(deleteTarget.id, {
      onSuccess: () => {
        toast('success', 'Profile deleted')
        setDeleteTarget(null)
        if (selectedId === deleteTarget.id) setSelectedId(null)
      },
      onError: (e) => toast('error', 'Delete failed', (e as Error).message),
    })
  }

  const columns = [
    profileCol.accessor('name', {
      header: 'Name',
      cell: (info) => <span className={styles.nameCell}>{info.getValue()}</span>,
    }),
    profileCol.accessor('namespace', {
      header: 'Namespace',
      cell: (info) => <span className={styles.monoCell}>{info.getValue()}</span>,
    }),
    profileCol.accessor('tenant', {
      header: 'Tenant',
      cell: (info) => <span className={styles.monoCell}>{info.getValue()}</span>,
    }),
    profileCol.accessor('fields', {
      header: 'Fields',
      cell: (info) => (
        <span className={styles.monoCell}>
          {Object.keys(info.getValue()).length}
        </span>
      ),
    }),
    profileCol.accessor('description', {
      header: 'Description',
      cell: (info) => (
        <span className={styles.descCell}>{info.getValue() ?? '-'}</span>
      ),
    }),
    profileCol.accessor('updated_at', {
      header: 'Updated',
      cell: (info) => relativeTime(info.getValue()),
    }),
    profileCol.display({
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
              icon={<Pencil className="h-3.5 w-3.5" />}
              onClick={() => setEditTarget(row)}
              aria-label="Edit profile"
            >
              Edit
            </Button>
            <Button
              variant="ghost"
              size="sm"
              icon={<Trash2 className="h-3.5 w-3.5" />}
              onClick={() => setDeleteTarget(row)}
              aria-label="Delete profile"
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
      <div className={styles.filterBar}>
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
        <Button
          icon={<Plus className="h-3.5 w-3.5" />}
          onClick={() => setShowCreate(true)}
        >
          Create Profile
        </Button>
      </div>

      <DataTable
        data={data?.profiles ?? []}
        columns={columns}
        loading={isLoading}
        onRowClick={(row) => setSelectedId(row.id)}
        emptyTitle="No profiles"
        emptyDescription="Create a template profile to compose named fields from templates."
      />

      {/* Create modal */}
      <ProfileFormModal
        key="create"
        open={showCreate}
        onClose={() => setShowCreate(false)}
        onSubmit={(req) => {
          createMutation.mutate(req, {
            onSuccess: (res) => {
              toast('success', 'Profile created', `ID: ${res.id}`)
              setShowCreate(false)
            },
            onError: (e) => toast('error', 'Create failed', (e as Error).message),
          })
        }}
        loading={createMutation.isPending}
        title="Create Profile"
      />

      {/* Edit modal */}
      {editTarget && (
        <ProfileFormModal
          key={editTarget.id}
          open={!!editTarget}
          onClose={() => setEditTarget(null)}
          onSubmit={(req) => {
            const body: UpdateProfileRequest = {
              fields: req.fields,
              description: req.description,
              labels: req.labels,
            }
            updateMutation.mutate({ id: editTarget.id, body }, {
              onSuccess: () => {
                toast('success', 'Profile updated')
                setEditTarget(null)
              },
              onError: (e) => toast('error', 'Update failed', (e as Error).message),
            })
          }}
          loading={updateMutation.isPending}
          title="Edit Profile"
          initial={editTarget}
        />
      )}

      {/* Detail drawer */}
      <Drawer
        open={!!selectedId}
        onClose={() => setSelectedId(null)}
        title={selectedProfile?.name ?? selectedId ?? ''}
        wide
      >
        {selectedProfile && (
          <ProfileDetailView
            profile={selectedProfile}
            onEdit={() => {
              setEditTarget(selectedProfile)
              setSelectedId(null)
            }}
            onDelete={() => setDeleteTarget(selectedProfile)}
          />
        )}
      </Drawer>

      {/* Delete confirmation modal */}
      <Modal
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        title="Delete Profile"
        size="sm"
        footer={
          <>
            <Button variant="secondary" onClick={() => setDeleteTarget(null)}>
              Cancel
            </Button>
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
          Are you sure you want to delete profile{' '}
          <span className={styles.deleteName}>{deleteTarget?.name}</span>? This
          cannot be undone.
        </p>
      </Modal>
    </div>
  )
}

// ---- Profile Form Modal ----

interface FieldEntry {
  key: string
  valueType: 'inline' | 'ref'
  inlineValue: string
  refValue: string
}

function fieldsToEntries(
  fields: Record<string, TemplateProfileField>,
): FieldEntry[] {
  return Object.entries(fields).map(([key, val]) => {
    if (typeof val === 'string') {
      return { key, valueType: 'inline', inlineValue: val, refValue: '' }
    }
    return { key, valueType: 'ref', inlineValue: '', refValue: val.$ref }
  })
}

function entriesToFields(
  entries: FieldEntry[],
): Record<string, TemplateProfileField> {
  const result: Record<string, TemplateProfileField> = {}
  for (const entry of entries) {
    if (!entry.key) continue
    if (entry.valueType === 'ref') {
      result[entry.key] = { $ref: entry.refValue }
    } else {
      result[entry.key] = entry.inlineValue
    }
  }
  return result
}

function ProfileFormModal({
  open,
  onClose,
  onSubmit,
  loading,
  title,
  initial,
}: {
  open: boolean
  onClose: () => void
  onSubmit: (req: CreateProfileRequest) => void
  loading: boolean
  title: string
  initial?: TemplateProfile
}) {
  const isEdit = !!initial

  const [name, setName] = useState(initial?.name ?? '')
  const [ns, setNs] = useState(initial?.namespace ?? '')
  const [tenant, setTenant] = useState(initial?.tenant ?? '')
  const [description, setDescription] = useState(initial?.description ?? '')
  const [labelsText, setLabelsText] = useState(
    initial?.labels
      ? Object.entries(initial.labels)
          .map(([k, v]) => `${k}=${v}`)
          .join('\n')
      : '',
  )
  const [fieldEntries, setFieldEntries] = useState<FieldEntry[]>(
    initial?.fields ? fieldsToEntries(initial.fields) : [emptyEntry()],
  )

  function emptyEntry(): FieldEntry {
    return { key: '', valueType: 'inline', inlineValue: '', refValue: '' }
  }

  const addField = () => setFieldEntries((prev) => [...prev, emptyEntry()])

  const removeField = (idx: number) =>
    setFieldEntries((prev) => prev.filter((_, i) => i !== idx))

  const updateField = (idx: number, patch: Partial<FieldEntry>) =>
    setFieldEntries((prev) =>
      prev.map((entry, i) => (i === idx ? { ...entry, ...patch } : entry)),
    )

  const handleSubmit = () => {
    const labels: Record<string, string> = {}
    for (const line of labelsText.split('\n')) {
      const trimmed = line.trim()
      if (!trimmed) continue
      const eqIdx = trimmed.indexOf('=')
      if (eqIdx > 0) {
        labels[trimmed.slice(0, eqIdx).trim()] = trimmed.slice(eqIdx + 1).trim()
      }
    }

    onSubmit({
      name,
      namespace: ns,
      tenant,
      fields: entriesToFields(fieldEntries),
      description: description || undefined,
      labels: Object.keys(labels).length > 0 ? labels : undefined,
    })
  }

  const isValid =
    name &&
    ns &&
    tenant &&
    fieldEntries.some((e) => e.key.trim())

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={title}
      size="lg"
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button
            loading={loading}
            onClick={handleSubmit}
            disabled={!isValid}
            icon={<Link className="h-3.5 w-3.5" />}
          >
            {isEdit ? 'Update' : 'Create'}
          </Button>
        </>
      }
    >
      <div className={styles.formSection}>
        <div className={styles.formGrid}>
          <Input
            label="Name *"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="email-alert-profile"
            disabled={isEdit}
          />
          <Input
            label="Description"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="Brief description"
          />
        </div>

        <div className={styles.formGrid}>
          <Input
            label="Namespace *"
            value={ns}
            onChange={(e) => setNs(e.target.value)}
            placeholder="prod"
            disabled={isEdit}
          />
          <Input
            label="Tenant *"
            value={tenant}
            onChange={(e) => setTenant(e.target.value)}
            placeholder="acme"
            disabled={isEdit}
          />
        </div>

        {/* Dynamic field builder */}
        <div>
          <label className={styles.textareaLabel}>Fields *</label>
          <div className={styles.fieldBuilderList} role="list" aria-label="Profile fields">
            {fieldEntries.map((entry, idx) => (
              <div
                key={idx}
                className={styles.fieldRow}
                role="listitem"
              >
                {/* Field key */}
                <div className={styles.fieldRowKey}>
                  <Input
                    placeholder="field_name"
                    value={entry.key}
                    onChange={(e) => updateField(idx, { key: e.target.value })}
                    aria-label={`Field ${idx + 1} name`}
                  />
                </div>

                {/* Toggle between inline / $ref */}
                <div className={styles.fieldTypeToggle}>
                  <button
                    type="button"
                    className={styles.fieldTypeBadge}
                    onClick={() =>
                      updateField(idx, {
                        valueType: entry.valueType === 'inline' ? 'ref' : 'inline',
                      })
                    }
                    aria-label={`Toggle field ${idx + 1} type (current: ${entry.valueType})`}
                    title="Click to toggle between inline value and template reference"
                  >
                    {entry.valueType === 'ref' ? '$ref' : 'inline'}
                  </button>
                </div>

                {/* Value input */}
                <div className={styles.fieldRowValue}>
                  {entry.valueType === 'ref' ? (
                    <Input
                      placeholder="template-name"
                      value={entry.refValue}
                      onChange={(e) => updateField(idx, { refValue: e.target.value })}
                      aria-label={`Field ${idx + 1} template reference`}
                    />
                  ) : (
                    <Input
                      placeholder="inline value or {{variable}}"
                      value={entry.inlineValue}
                      onChange={(e) =>
                        updateField(idx, { inlineValue: e.target.value })
                      }
                      aria-label={`Field ${idx + 1} inline value`}
                    />
                  )}
                </div>

                {/* Remove field */}
                <Button
                  variant="ghost"
                  size="sm"
                  icon={<Trash2 className="h-3.5 w-3.5" />}
                  onClick={() => removeField(idx)}
                  aria-label={`Remove field ${idx + 1}`}
                  disabled={fieldEntries.length === 1}
                />
              </div>
            ))}
          </div>

          <div className={styles.addFieldButton}>
            <Button
              variant="secondary"
              size="sm"
              icon={<Plus className="h-3.5 w-3.5" />}
              onClick={addField}
            >
              Add Field
            </Button>
          </div>
        </div>

        <div>
          <label className={styles.textareaLabel} htmlFor="profile-labels">
            Labels (key=value, one per line)
          </label>
          <textarea
            id="profile-labels"
            className={styles.textareaShort}
            value={labelsText}
            onChange={(e) => setLabelsText(e.target.value)}
            placeholder={"team=platform\nenv=prod"}
          />
        </div>
      </div>
    </Modal>
  )
}

// ---- Profile Detail View ----

function ProfileDetailView({
  profile,
  onEdit,
  onDelete,
}: {
  profile: TemplateProfile
  onEdit: () => void
  onDelete: () => void
}) {
  return (
    <div>
      <div className={styles.detailContent}>
        {(
          [
            ['ID', profile.id],
            ['Name', profile.name],
            ['Namespace', profile.namespace],
            ['Tenant', profile.tenant],
            ['Description', profile.description ?? '-'],
            ['Fields', String(Object.keys(profile.fields).length)],
            ['Created', relativeTime(profile.created_at)],
            ['Updated', relativeTime(profile.updated_at)],
          ] as [string, string][]
        ).map(([k, v]) => (
          <div key={k} className={styles.detailRow}>
            <span className={styles.detailLabel}>{k}</span>
            <span className={styles.detailValueWrap}>{v}</span>
          </div>
        ))}
      </div>

      {profile.labels && Object.keys(profile.labels).length > 0 && (
        <div>
          <h2 className={styles.sectionTitle}>Labels</h2>
          <div className={styles.detailContent}>
            {Object.entries(profile.labels).map(([k, v]) => (
              <div key={k} className={styles.detailRow}>
                <span className={styles.detailLabel}>{k}</span>
                <span className={styles.detailValue}>{v}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      <h2 className={styles.sectionTitle}>Fields</h2>
      <div className={styles.fieldsList} role="list" aria-label="Profile fields">
        {Object.entries(profile.fields).map(([key, val]) => {
          const isRef = typeof val !== 'string'
          return (
            <div key={key} className={styles.fieldsEntry} role="listitem">
              <span className={styles.fieldsKey}>{key}</span>
              {isRef ? (
                <span className={styles.refBadge} aria-label={`Template reference: ${(val as { $ref: string }).$ref}`}>
                  $ref: {(val as { $ref: string }).$ref}
                </span>
              ) : (
                <span className={styles.fieldsValue}>{val as string}</span>
              )}
            </div>
          )
        })}
      </div>

      <div className={styles.actionButtons}>
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
  )
}

// ---- Playground Tab ----

function tryParseJson(text: string): { ok: true; value: Record<string, unknown> } | { ok: false; error: string } {
  try {
    const parsed = JSON.parse(text)
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
      return { ok: false, error: 'Payload must be a JSON object' }
    }
    return { ok: true, value: parsed }
  } catch (e) {
    return { ok: false, error: (e as Error).message }
  }
}

function PlaygroundTab() {
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

          <div className={styles.formGrid}>
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
            <label className={styles.textareaLabel} htmlFor="playground-profile">
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
            <label className={styles.textareaLabel} htmlFor="playground-payload">
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
