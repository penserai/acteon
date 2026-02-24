import { useState } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import { Plus, Pencil, Trash2, Link } from 'lucide-react'
import {
  useTemplateProfiles,
  useTemplateProfile,
  useCreateProfile,
  useUpdateProfile,
  useDeleteProfile,
} from '../../api/hooks/useTemplates'
import { DataTable } from '../../components/ui/DataTable'
import { Button } from '../../components/ui/Button'
import { Input } from '../../components/ui/Input'
import { Modal } from '../../components/ui/Modal'
import { DeleteConfirmModal } from '../../components/ui/DeleteConfirmModal'
import { Drawer } from '../../components/ui/Drawer'
import { useToast } from '../../components/ui/useToast'
import { relativeTime, parseLabels, labelsToText } from '../../lib/format'
import type {
  TemplateProfile,
  CreateProfileRequest,
  UpdateProfileRequest,
} from '../../types'
import { type FieldEntry, emptyEntry, fieldsToEntries, entriesToFields } from './profileUtils'
import shared from '../../styles/shared.module.css'
import styles from '../Templates.module.css'

const profileCol = createColumnHelper<TemplateProfile>()

export function ProfilesTab() {
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
            className={shared.actionsCell}
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
      <div className={shared.filterBar}>
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
      <DeleteConfirmModal
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={handleDelete}
        loading={deleteMutation.isPending}
        title="Delete Profile"
        name={deleteTarget?.name ?? ''}
      />
    </div>
  )
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
    initial?.labels ? labelsToText(initial.labels) : '',
  )
  const [fieldEntries, setFieldEntries] = useState<FieldEntry[]>(
    initial?.fields ? fieldsToEntries(initial.fields) : [emptyEntry()],
  )

  const addField = () => setFieldEntries((prev) => [...prev, emptyEntry()])

  const removeField = (idx: number) =>
    setFieldEntries((prev) => prev.filter((_, i) => i !== idx))

  const updateField = (idx: number, patch: Partial<FieldEntry>) =>
    setFieldEntries((prev) =>
      prev.map((entry, i) => (i === idx ? { ...entry, ...patch } : entry)),
    )

  const handleSubmit = () => {
    const labels = parseLabels(labelsText)
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
      <div className={shared.formSection}>
        <div className={shared.formGrid}>
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

        <div className={shared.formGrid}>
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
          <label className={shared.textareaLabel}>Fields *</label>
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
          <label className={shared.textareaLabel} htmlFor="profile-labels">
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
          <div key={k} className={shared.detailRow}>
            <span className={shared.detailLabel}>{k}</span>
            <span className={styles.detailValueWrap}>{v}</span>
          </div>
        ))}
      </div>

      {profile.labels && Object.keys(profile.labels).length > 0 && (
        <div>
          <h2 className={shared.sectionTitle}>Labels</h2>
          <div className={styles.detailContent}>
            {Object.entries(profile.labels).map(([k, v]) => (
              <div key={k} className={shared.detailRow}>
                <span className={shared.detailLabel}>{k}</span>
                <span className={shared.detailValue}>{v}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      <h2 className={shared.sectionTitle}>Fields</h2>
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
  )
}
