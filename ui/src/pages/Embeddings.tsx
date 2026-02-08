import { useState } from 'react'
import { Search } from 'lucide-react'
import { apiPost } from '../api/client'
import { PageHeader } from '../components/layout/PageHeader'
import { Input } from '../components/ui/Input'
import { Button } from '../components/ui/Button'
import { useToast } from '../components/ui/Toast'
import type { SimilarityResponse } from '../types'
import styles from './Embeddings.module.css'

export function Embeddings() {
  const [text, setText] = useState('')
  const [topic, setTopic] = useState('')
  const [result, setResult] = useState<SimilarityResponse | null>(null)
  const [loading, setLoading] = useState(false)
  const { toast } = useToast()

  const handleTest = async () => {
    if (!text || !topic) return
    setLoading(true)
    try {
      const res = await apiPost<SimilarityResponse>('/v1/embeddings/similarity', { text, topic })
      setResult(res)
    } catch (e) {
      toast('error', 'Similarity failed', (e as Error).message)
    } finally {
      setLoading(false)
    }
  }

  return (
    <div>
      <PageHeader title="Embeddings" subtitle="Test semantic similarity scoring" />

      <div className={styles.container}>
        <div className={styles.formCard}>
          <Input label="Text" value={text} onChange={(e) => setText(e.target.value)} placeholder="The action payload text..." />
          <Input label="Topic" value={topic} onChange={(e) => setTopic(e.target.value)} placeholder="security-alert" />
          <Button
            icon={<Search className="h-3.5 w-3.5" />}
            loading={loading}
            onClick={() => void handleTest()}
            disabled={!text || !topic}
          >
            Compute Similarity
          </Button>
        </div>

        {result && (
          <div className={styles.resultCard}>
            <p className={styles.resultLabel}>Cosine Similarity</p>
            <p className={styles.resultValue}>{result.similarity.toFixed(4)}</p>
            <p className={styles.resultTopic}>Topic: {result.topic}</p>
          </div>
        )}
      </div>
    </div>
  )
}
