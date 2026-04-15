import { lazy, useEffect, Suspense } from 'react'
import { Routes, Route } from 'react-router-dom'
import { AppShell } from './components/layout/AppShell'
import { ToastProvider } from './components/ui/Toast'
import { CommandPalette } from './components/command-palette/CommandPalette'
import { connectEvents, disconnectEvents } from './stores/events'
import { Dashboard } from './pages/Dashboard'

// Every non-Dashboard route is lazy-loaded so its JS, CSS, and the
// libraries it pulls in (recharts, @xyflow/react, codemirror, etc.)
// only ship to the browser when the user actually navigates there.
// Dashboard stays eager because it's the index/landing page.
const Dispatch = lazy(() => import('./pages/Dispatch').then((m) => ({ default: m.Dispatch })))
const Rules = lazy(() => import('./pages/Rules').then((m) => ({ default: m.Rules })))
const Actions = lazy(() => import('./pages/Actions').then((m) => ({ default: m.Actions })))
const Events = lazy(() => import('./pages/Events').then((m) => ({ default: m.Events })))
const Groups = lazy(() => import('./pages/Groups').then((m) => ({ default: m.Groups })))
const Silences = lazy(() => import('./pages/Silences').then((m) => ({ default: m.Silences })))
const TimeIntervals = lazy(() =>
  import('./pages/TimeIntervals').then((m) => ({ default: m.TimeIntervals })),
)
const Alerting = lazy(() => import('./pages/Alerting').then((m) => ({ default: m.Alerting })))
const Chains = lazy(() => import('./pages/Chains').then((m) => ({ default: m.Chains })))
const ChainDetail = lazy(() =>
  import('./pages/ChainDetail').then((m) => ({ default: m.ChainDetail })),
)
const Approvals = lazy(() => import('./pages/Approvals').then((m) => ({ default: m.Approvals })))
const Providers = lazy(() => import('./pages/Providers').then((m) => ({ default: m.Providers })))
const ProviderHealth = lazy(() =>
  import('./pages/ProviderHealth').then((m) => ({ default: m.ProviderHealth })),
)
const DeadLetterQueue = lazy(() =>
  import('./pages/DeadLetterQueue').then((m) => ({ default: m.DeadLetterQueue })),
)
const EventStream = lazy(() =>
  import('./pages/EventStream').then((m) => ({ default: m.EventStream })),
)
const Embeddings = lazy(() => import('./pages/Embeddings').then((m) => ({ default: m.Embeddings })))
const ScheduledActions = lazy(() =>
  import('./pages/ScheduledActions').then((m) => ({ default: m.ScheduledActions })),
)
const RecurringActions = lazy(() =>
  import('./pages/RecurringActions').then((m) => ({ default: m.RecurringActions })),
)
const Quotas = lazy(() => import('./pages/Quotas').then((m) => ({ default: m.Quotas })))
const RetentionPolicies = lazy(() =>
  import('./pages/RetentionPolicies').then((m) => ({ default: m.RetentionPolicies })),
)
const RulePlayground = lazy(() =>
  import('./pages/RulePlayground').then((m) => ({ default: m.RulePlayground })),
)
const WasmPlugins = lazy(() =>
  import('./pages/WasmPlugins').then((m) => ({ default: m.WasmPlugins })),
)
const Templates = lazy(() => import('./pages/Templates').then((m) => ({ default: m.Templates })))
const ChainDefinitions = lazy(() =>
  import('./pages/ChainDefinitions').then((m) => ({ default: m.ChainDefinitions })),
)
const Analytics = lazy(() => import('./pages/Analytics').then((m) => ({ default: m.Analytics })))
const ComplianceStatus = lazy(() =>
  import('./pages/ComplianceStatus').then((m) => ({ default: m.ComplianceStatus })),
)
const Settings = lazy(() => import('./pages/Settings').then((m) => ({ default: m.Settings })))

function RouteFallback() {
  return (
    <div
      style={{
        padding: '2rem',
        color: 'var(--text-muted)',
        fontSize: '0.875rem',
      }}
      role="status"
      aria-live="polite"
    >
      Loading…
    </div>
  )
}

function App() {
  useEffect(() => {
    connectEvents()
    return () => disconnectEvents()
  }, [])

  return (
    <ToastProvider>
      <CommandPalette />
      <Suspense fallback={<RouteFallback />}>
        <Routes>
          <Route element={<AppShell />}>
            <Route index element={<Dashboard />} />
            <Route path="alerting" element={<Alerting />} />
            <Route path="dispatch" element={<Dispatch />} />
            <Route path="rules" element={<Rules />} />
            <Route path="playground" element={<RulePlayground />} />
            <Route path="audit" element={<Actions />} />
            <Route path="events" element={<Events />} />
            <Route path="groups" element={<Groups />} />
            <Route path="silences" element={<Silences />} />
            <Route path="time-intervals" element={<TimeIntervals />} />
            <Route path="chains" element={<Chains />} />
            <Route path="chains/:chainId" element={<ChainDetail />} />
            <Route path="chain-definitions" element={<ChainDefinitions />} />
            <Route path="approvals" element={<Approvals />} />
            <Route path="circuit-breakers" element={<Providers />} />
            <Route path="provider-health" element={<ProviderHealth />} />
            <Route path="dlq" element={<DeadLetterQueue />} />
            <Route path="stream" element={<EventStream />} />
            <Route path="embeddings" element={<Embeddings />} />
            <Route path="scheduled" element={<ScheduledActions />} />
            <Route path="recurring" element={<RecurringActions />} />
            <Route path="quotas" element={<Quotas />} />
            <Route path="retention" element={<RetentionPolicies />} />
            <Route path="wasm-plugins" element={<WasmPlugins />} />
            <Route path="templates" element={<Templates />} />
            <Route path="analytics" element={<Analytics />} />
            <Route path="compliance" element={<ComplianceStatus />} />
            <Route path="settings/*" element={<Settings />} />
          </Route>
        </Routes>
      </Suspense>
    </ToastProvider>
  )
}

export default App
