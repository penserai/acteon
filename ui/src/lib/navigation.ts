import {
  LayoutDashboard, Send, BookOpen, FlaskConical, ScrollText, Radio, Layers, Link2, ShieldCheck,
  Zap, AlertTriangle, Rss, Brain, Gauge, Users, Server, Cpu, Eye, Settings,
  RefreshCw, PieChart, Database, HeartPulse, Puzzle, ShieldAlert, FileText, BarChart3,
  ExternalLink
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'

export interface NavItem {
  to: string
  label: string
  icon: LucideIcon
  shortcut?: string
  external?: boolean
  end?: boolean
}

export const MAIN_NAV_ITEMS: NavItem[] = [
  { to: '/', icon: LayoutDashboard, label: 'Dashboard', shortcut: '1', end: true },
  { to: '/dispatch', icon: Send, label: 'Dispatch' },
  { to: '/rules', icon: BookOpen, label: 'Rules', shortcut: '2' },
  { to: '/playground', icon: FlaskConical, label: 'Rule Playground' },
  { to: '/audit', icon: ScrollText, label: 'Audit Trail', shortcut: '4' },
  { to: '/events', icon: Radio, label: 'Events' },
  { to: '/groups', icon: Layers, label: 'Groups' },
  { to: '/chains', icon: Link2, label: 'Chains', shortcut: '3' },
  { to: '/approvals', icon: ShieldCheck, label: 'Approvals', shortcut: '5' },
  { to: '/circuit-breakers', icon: Zap, label: 'Circuit Breakers' },
  { to: '/provider-health', icon: HeartPulse, label: 'Provider Health' },
  { to: '/analytics', icon: BarChart3, label: 'Analytics' },
  { to: '/dlq', icon: AlertTriangle, label: 'Dead-Letter Queue' },
  { to: '/stream', icon: Rss, label: 'Stream' },
  { to: '/embeddings', icon: Brain, label: 'Embeddings' },
  { to: '/recurring', icon: RefreshCw, label: 'Recurring Actions' },
  { to: '/quotas', icon: PieChart, label: 'Quotas' },
  { to: '/retention', icon: Database, label: 'Retention' },
  { to: '/wasm-plugins', icon: Puzzle, label: 'WASM Plugins' },
  { to: '/templates', icon: FileText, label: 'Templates' },
  { to: '/compliance', icon: ShieldAlert, label: 'Compliance' },
  {
    to: 'https://penserai.github.io/acteon',
    icon: ExternalLink,
    label: 'Documentation',
    external: true,
  },
]

export const SETTINGS_NAV_ITEMS: NavItem[] = [
  { to: '/settings/rate-limiting', icon: Gauge, label: 'Rate Limiting' },
  { to: '/settings/auth', icon: Users, label: 'Auth & Users' },
  { to: '/settings/providers', icon: Server, label: 'Providers' },
  { to: '/settings/llm', icon: Cpu, label: 'LLM Guardrail' },
  { to: '/settings/telemetry', icon: Eye, label: 'Telemetry' },
  { to: '/settings/config', icon: Settings, label: 'Server Config' },
  { to: '/settings/background', icon: Cpu, label: 'Background Tasks' },
]

export const ALL_NAV_ITEMS = [...MAIN_NAV_ITEMS, ...SETTINGS_NAV_ITEMS]

export const ROUTE_NAMES = ALL_NAV_ITEMS.reduce<Record<string, string>>((acc, item) => {
  acc[item.to] = item.label
  return acc
}, { '/settings': 'Settings' })
