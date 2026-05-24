// Phase 7 — single-conversation thread view.
//
// Drilldown from `/bus?tab=conversations`. Shows:
//   - Conversation metadata (state, participants, events_topic, ...)
//   - Replayed message thread, color-coded by envelope kind:
//     plain conversation messages, tool_call / tool_result (6a),
//     stream_chunk / stream_end (6b), ordinary chats. Headers are
//     visible on each row so operators can inspect the routing
//     metadata that backed each record.
//
// V1 keeps the thread one-page; pagination via `next_cursor` is a
// natural future extension once threads grow long enough to need it.

import { useParams } from 'react-router-dom'
import { Link } from 'react-router-dom'

import { PageHeader } from '../components/layout/PageHeader'
import { Badge } from '../components/ui/Badge'
import { EmptyState } from '../components/ui/EmptyState'
import { useBusConversation, useBusConversationMessages } from '../api/hooks/useBus'
import { relativeTime } from '../lib/format'

import styles from './BusConversation.module.css'

export function BusConversation() {
  const { namespace, tenant, id } = useParams<{ namespace: string; tenant: string; id: string }>()
  const { data: conv, isLoading } = useBusConversation(namespace, tenant, id)
  const { data: replay, isLoading: replayLoading } = useBusConversationMessages(namespace, tenant, id, {
    limit: 200,
  })

  if (isLoading || !conv) {
    return (
      <div>
        <PageHeader title="Conversation" />
        <p className={styles.timestamp}>Loading…</p>
      </div>
    )
  }

  return (
    <div>
      <PageHeader
        title={conv.conversation_id}
        subtitle={`${conv.namespace} / ${conv.tenant}`}
        actions={
          <Link to="/bus?tab=conversations" className={styles.headerLine}>
            ← Back to conversations
          </Link>
        }
      />
      <div className={styles.header}>
        <Badge>{conv.state}</Badge>
        {conv.participants.length === 0 ? (
          <span className={styles.timestamp}>open conversation</span>
        ) : (
          conv.participants.map((p) => <Badge key={p}>{p}</Badge>)
        )}
        <span className={styles.timestamp}>
          updated {relativeTime(conv.updated_at)}
        </span>
      </div>
      {conv.events_topic && (
        <div className={styles.headerLine}>
          events_topic override:{' '}
          <span className={styles.idCell}>{conv.events_topic}</span>
        </div>
      )}
      {conv.description && <p className="text-sm mt-2">{conv.description}</p>}

      {replayLoading ? (
        <p className={`${styles.timestamp} mt-4`}>Loading thread…</p>
      ) : !replay || replay.messages.length === 0 ? (
        <EmptyState
          title="No messages"
          description="The conversation hasn't received any messages yet, or the events topic was reset."
        />
      ) : (
        <div className={styles.threadList}>
          {replay.messages.map((msg, i) => {
            const kind = msg.headers['acteon.envelope.kind'] ?? 'message'
            const approvalId = msg.headers['acteon.approval.id']
            const callId = msg.headers['acteon.tool.call_id']
            const streamId = msg.headers['acteon.stream.id']
            return (
              <article key={`${msg.partition}:${msg.offset}:${i}`} className={styles.message}>
                <div className={styles.messageHeader}>
                  <div className="flex items-baseline gap-2">
                    <Badge>{kind}</Badge>
                    {msg.sender && (
                      <span className={styles.idCell}>{msg.sender}</span>
                    )}
                    {callId && (
                      <span className={styles.headerLine}>call_id: {callId}</span>
                    )}
                    {streamId && (
                      <span className={styles.headerLine}>stream_id: {streamId}</span>
                    )}
                    {approvalId && (
                      <Link
                        to={`/bus?tab=approvals&ns=${namespace}&tenant=${tenant}&approval_id=${encodeURIComponent(approvalId)}`}
                        className={styles.headerLine}
                      >
                        approval: {approvalId.slice(0, 8)}…
                      </Link>
                    )}
                  </div>
                  <span className={styles.timestamp}>
                    p{msg.partition}/o{msg.offset} • {relativeTime(msg.timestamp)}
                  </span>
                </div>
                <pre className={styles.payload}>
                  {JSON.stringify(msg.payload, null, 2)}
                </pre>
              </article>
            )
          })}
        </div>
      )}
    </div>
  )
}
