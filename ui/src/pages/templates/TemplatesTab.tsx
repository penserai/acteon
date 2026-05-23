import { useState } from 'react'
import { createColumnHelper } from '@tanstack/react-table'
import { Plus, Pencil, Trash2, FileText } from 'lucide-react'
import {
  useTemplates,
  useTemplate,
  useCreateTemplate,
  useUpdateTemplate,
  useDeleteTemplate,
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
  Template,
  CreateTemplateRequest,
  UpdateTemplateRequest,
} from '../../types'
import shared from '../../styles/shared.module.css'
import styles from '../Templates.module.css'

const templateCol = createColumnHelper<Template>()

export function TemplatesTab() {
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
      <DeleteConfirmModal
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={handleDelete}
        loading={deleteMutation.isPending}
        title="Delete Template"
        name={deleteTarget?.name ?? ''}
        warning="Any profiles referencing this template will break."
      />
    </div>
  )
}

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
    initial?.labels ? labelsToText(initial.labels) : '',
  )

  const handleSubmit = () => {
    const labels = parseLabels(labelsText)
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
      <div className={shared.formSection}>
        <div className={shared.formGrid}>
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

        <div>
          <label className={shared.textareaLabel} htmlFor="template-content">
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
          <label className={shared.textareaLabel} htmlFor="template-labels">
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
          <div key={k} className={shared.detailRow}>
            <span className={shared.detailLabel}>{k}</span>
            <span className={styles.detailValueWrap}>{v}</span>
          </div>
        ))}
      </div>

      {template.labels && Object.keys(template.labels).length > 0 && (
        <div>
          <h2 className={shared.sectionTitle}>Labels</h2>
          <div className={styles.detailContent}>
            {Object.entries(template.labels).map(([k, v]) => (
              <div key={k} className={shared.detailRow}>
                <span className={shared.detailLabel}>{k}</span>
                <span className={shared.detailValue}>{v}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      <h2 className={shared.sectionTitle}>Content</h2>
      <pre className={styles.contentPreview}>{template.content}</pre>

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
