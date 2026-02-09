import { cn } from '../../lib/cn'
import styles from './Toggle.module.css'

interface ToggleProps {
  checked: boolean
  onChange: (checked: boolean) => void
  label?: string
  disabled?: boolean
}

export function Toggle({ checked, onChange, label, disabled }: ToggleProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        styles.toggle,
        checked ? styles.toggleChecked : styles.toggleUnchecked,
        disabled && styles.toggleDisabled,
      )}
    >
      <span
        className={cn(
          styles.thumb,
          checked ? styles.thumbChecked : styles.thumbUnchecked,
        )}
      />
    </button>
  )
}
