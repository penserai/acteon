import { useLocation, Navigate } from 'react-router-dom'
import { PageHeader } from '../components/layout/PageHeader'
import { SettingsRateLimiting } from './settings/RateLimiting'
import { SettingsAuth } from './settings/Auth'
import { SettingsProviders } from './settings/Providers'
import { SettingsLlm } from './settings/Llm'
import { SettingsTelemetry } from './settings/Telemetry'
import { SettingsServerConfig } from './settings/ServerConfig'
import { SettingsBackground } from './settings/Background'

const settingsRoutes: Record<string, { title: string; component: React.ComponentType }> = {
  'rate-limiting': { title: 'Rate Limiting', component: SettingsRateLimiting },
  'auth': { title: 'Auth & Users', component: SettingsAuth },
  'providers': { title: 'Providers', component: SettingsProviders },
  'llm': { title: 'LLM Guardrail', component: SettingsLlm },
  'telemetry': { title: 'Telemetry', component: SettingsTelemetry },
  'config': { title: 'Server Config', component: SettingsServerConfig },
  'background': { title: 'Background Tasks', component: SettingsBackground },
}

export function Settings() {
  const location = useLocation()
  const subPath = location.pathname.replace('/settings/', '').replace('/settings', '')

  if (!subPath || !settingsRoutes[subPath]) {
    return <Navigate to="/settings/config" replace />
  }

  const { title, component: Component } = settingsRoutes[subPath]

  return (
    <div>
      <PageHeader title={title} />
      <Component />
    </div>
  )
}
