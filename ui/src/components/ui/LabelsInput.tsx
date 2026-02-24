import { useState, useCallback } from 'react'
import { parseLabels, labelsToText } from '../../lib/format'
import shared from '../../styles/shared.module.css'

interface LabelsInputProps {
  value: Record<string, string>
  onChange: (labels: Record<string, string>) => void
  id?: string
}

export function LabelsInput({ value, onChange, id }: LabelsInputProps) {
  const [text, setText] = useState(() => labelsToText(value))

  const handleChange = useCallback(
    (e: React.ChangeEvent<HTMLTextAreaElement>) => {
      const newText = e.target.value
      setText(newText)
      onChange(parseLabels(newText))
    },
    [onChange],
  )

  return (
    <div>
      <label className={shared.textareaLabel} htmlFor={id}>
        Labels (key=value, one per line)
      </label>
      <textarea
        id={id}
        value={text}
        onChange={handleChange}
        className={shared.textarea}
        placeholder={'team=platform\nenv=prod'}
      />
    </div>
  )
}
