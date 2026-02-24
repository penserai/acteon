import { useEffect } from 'react'
import { Routes, Route } from 'react-router-dom'
import { AppShell } from './components/layout/AppShell'
import { ToastProvider } from './components/ui/Toast'
import { CommandPalette } from './components/command-palette/CommandPalette'
import { connectEvents, disconnectEvents } from './stores/events'
import { Dashboard } from './pages/Dashboard'
import { Dispatch } from './pages/Dispatch'
import { Rules } from './pages/Rules'
import { Actions } from './pages/Actions'
import { Events } from './pages/Events'
import { Groups } from './pages/Groups'
import { Chains } from './pages/Chains'
import { ChainDetail } from './pages/ChainDetail'
import { Approvals } from './pages/Approvals'
import { Providers } from './pages/Providers'
import { ProviderHealth } from './pages/ProviderHealth'
import { DeadLetterQueue } from './pages/DeadLetterQueue'
import { EventStream } from './pages/EventStream'
import { Embeddings } from './pages/Embeddings'
import { ScheduledActions } from './pages/ScheduledActions'
import { RecurringActions } from './pages/RecurringActions'
import { Quotas } from './pages/Quotas'
import { RetentionPolicies } from './pages/RetentionPolicies'
import { RulePlayground } from './pages/RulePlayground'
import { WasmPlugins } from './pages/WasmPlugins'
import { Templates } from './pages/Templates'
import { Analytics } from './pages/Analytics'
import { ComplianceStatus } from './pages/ComplianceStatus'
import { Settings } from './pages/Settings'

function App() {
  useEffect(() => {
    connectEvents()
    return () => disconnectEvents()
  }, [])

  return (
    <ToastProvider>
      <CommandPalette />
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<Dashboard />} />
          <Route path="dispatch" element={<Dispatch />} />
          <Route path="rules" element={<Rules />} />
          <Route path="playground" element={<RulePlayground />} />
          <Route path="audit" element={<Actions />} />
          <Route path="events" element={<Events />} />
          <Route path="groups" element={<Groups />} />
          <Route path="chains" element={<Chains />} />
          <Route path="chains/:chainId" element={<ChainDetail />} />
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
