import { lazy, useEffect } from 'react'
import type { ComponentType } from 'react'
import { Routes, Route } from 'react-router-dom'
import { AppShell } from './components/layout/AppShell'
import { ToastProvider } from './components/ui/Toast'
import { CommandPalette } from './components/command-palette/CommandPalette'
import { connectEvents, disconnectEvents } from './stores/events'
import { Dashboard } from './pages/Dashboard'

// -----------------------------------------------------------------------------
// lazyPage helper
// -----------------------------------------------------------------------------
// The project uses named exports for pages (e.g. `export function Dispatch`).
// React.lazy() expects a module with a `default` export, so each lazy import
// has to shim a `{ default: m.PageName }` object. `lazyPage` centralizes the
// shim so App.tsx's route table reads like a flat list instead of a wall of
// nearly-identical `.then()` callbacks.
//
// The generics give you a type-checked page name: if you typo
// `lazyPage(() => import('./pages/Dispatch'), 'Dispatchh')` TypeScript flags
// it at compile time because `'Dispatchh'` isn't a key of the module.

type PageModule<Name extends string> = {
  [K in Name]: ComponentType<unknown>
}

function lazyPage<Name extends string>(
  loader: () => Promise<PageModule<Name>>,
  name: Name,
) {
  return lazy(async () => {
    const mod = await loader()
    return { default: mod[name] }
  })
}

// -----------------------------------------------------------------------------
// Lazy route modules
// -----------------------------------------------------------------------------
// Every non-Dashboard route is lazy-loaded so its JS, CSS, and the libraries
// it pulls in (recharts, @xyflow/react, etc.) only ship to the browser when
// the user actually navigates there. Dashboard stays eager because it's the
// index/landing page — the first paint would otherwise show a spinner.

const Dispatch = lazyPage(() => import('./pages/Dispatch'), 'Dispatch')
const Rules = lazyPage(() => import('./pages/Rules'), 'Rules')
const Actions = lazyPage(() => import('./pages/Actions'), 'Actions')
const Events = lazyPage(() => import('./pages/Events'), 'Events')
const Groups = lazyPage(() => import('./pages/Groups'), 'Groups')
const Silences = lazyPage(() => import('./pages/Silences'), 'Silences')
const TimeIntervals = lazyPage(() => import('./pages/TimeIntervals'), 'TimeIntervals')
const Alerting = lazyPage(() => import('./pages/Alerting'), 'Alerting')
const Chains = lazyPage(() => import('./pages/Chains'), 'Chains')
const ChainDetail = lazyPage(() => import('./pages/ChainDetail'), 'ChainDetail')
const Approvals = lazyPage(() => import('./pages/Approvals'), 'Approvals')
const Providers = lazyPage(() => import('./pages/Providers'), 'Providers')
const ProviderHealth = lazyPage(() => import('./pages/ProviderHealth'), 'ProviderHealth')
const DeadLetterQueue = lazyPage(() => import('./pages/DeadLetterQueue'), 'DeadLetterQueue')
const EventStream = lazyPage(() => import('./pages/EventStream'), 'EventStream')
const Embeddings = lazyPage(() => import('./pages/Embeddings'), 'Embeddings')
const ScheduledActions = lazyPage(() => import('./pages/ScheduledActions'), 'ScheduledActions')
const RecurringActions = lazyPage(() => import('./pages/RecurringActions'), 'RecurringActions')
const Quotas = lazyPage(() => import('./pages/Quotas'), 'Quotas')
const RetentionPolicies = lazyPage(() => import('./pages/RetentionPolicies'), 'RetentionPolicies')
const RulePlayground = lazyPage(() => import('./pages/RulePlayground'), 'RulePlayground')
const WasmPlugins = lazyPage(() => import('./pages/WasmPlugins'), 'WasmPlugins')
const Templates = lazyPage(() => import('./pages/Templates'), 'Templates')
const ChainDefinitions = lazyPage(() => import('./pages/ChainDefinitions'), 'ChainDefinitions')
const Analytics = lazyPage(() => import('./pages/Analytics'), 'Analytics')
const ComplianceStatus = lazyPage(() => import('./pages/ComplianceStatus'), 'ComplianceStatus')
const Settings = lazyPage(() => import('./pages/Settings'), 'Settings')

function App() {
  useEffect(() => {
    connectEvents()
    return () => disconnectEvents()
  }, [])

  return (
    <ToastProvider>
      <CommandPalette />
      {/*
        The Suspense boundary for lazy route chunks lives inside
        AppShell (wrapping the <Outlet />), not here. That keeps the
        Sidebar + Header mounted during navigation and lets framer-
        motion's AnimatePresence run its exit animation before the
        RouteFallback spinner appears. See AppShell.tsx for details.
      */}
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
    </ToastProvider>
  )
}

export default App
