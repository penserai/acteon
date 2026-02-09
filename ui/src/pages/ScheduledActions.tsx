import { PageHeader } from '../components/layout/PageHeader'
import { EmptyState } from '../components/ui/EmptyState'
import { Clock } from 'lucide-react'

export function ScheduledActions() {
  return (
    <div>
      <PageHeader title="Scheduled Actions" />
      <EmptyState
        icon={<Clock className="h-12 w-12" />}
        title="No scheduled actions"
        description="Actions dispatched with starts_at will appear here until their scheduled time."
      />
    </div>
  )
}
