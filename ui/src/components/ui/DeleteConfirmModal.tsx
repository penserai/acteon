import { Modal } from './Modal'
import { Button } from './Button'
import shared from '../../styles/shared.module.css'

interface DeleteConfirmModalProps {
  open: boolean
  onClose: () => void
  onConfirm: () => void
  loading: boolean
  title: string
  name: string
  warning?: string
}

export function DeleteConfirmModal({
  open,
  onClose,
  onConfirm,
  loading,
  title,
  name,
  warning = 'This cannot be undone.',
}: DeleteConfirmModalProps) {
  return (
    <Modal
      open={open}
      onClose={onClose}
      title={title}
      size="sm"
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="danger" loading={loading} onClick={onConfirm}>
            Delete
          </Button>
        </>
      }
    >
      <p className={shared.deleteWarning}>
        Are you sure you want to delete{' '}
        <span className={shared.deleteName}>{name}</span>? {warning}
      </p>
    </Modal>
  )
}
