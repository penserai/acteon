/**
 * Playwright script to capture Sub-Chains documentation screenshots with realistic mock data.
 *
 * Usage: npx playwright test sub-chain-screenshots --project chromium
 */
import { test } from '@playwright/test'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)
const ASSETS = path.resolve(__dirname, '../../docs/screenshots')

// Use production build for reliable CSS rendering
const BASE_URL = 'http://localhost:4173'

test.use({ viewport: { width: 1440, height: 900 } })

// ---------------------------------------------------------------------------
// Mock data — Chain Executions (sub-chain scenario)
// ---------------------------------------------------------------------------

const now = new Date()
const ago = (min: number) => new Date(now.getTime() - min * 60_000).toISOString()

const CHAINS_LIST = {
  chains: [
    {
      chain_id: 'chn_a1b2c3d4e5f6',
      chain_name: 'incident-response',
      status: 'waiting_sub_chain',
      current_step: 2,
      total_steps: 4,
      started_at: ago(12),
      updated_at: ago(1),
    },
    {
      chain_id: 'chn_sub_esc_001',
      chain_name: 'escalation-runbook',
      status: 'running',
      current_step: 1,
      total_steps: 3,
      started_at: ago(1),
      updated_at: ago(0),
    },
    {
      chain_id: 'chn_f7e8d9c0b1a2',
      chain_name: 'deploy-pipeline',
      status: 'completed',
      current_step: 5,
      total_steps: 5,
      started_at: ago(45),
      updated_at: ago(30),
    },
    {
      chain_id: 'chn_1234abcd5678',
      chain_name: 'user-onboarding',
      status: 'completed',
      current_step: 3,
      total_steps: 3,
      started_at: ago(120),
      updated_at: ago(115),
    },
    {
      chain_id: 'chn_9988aabb7766',
      chain_name: 'data-sync',
      status: 'failed',
      current_step: 3,
      total_steps: 6,
      started_at: ago(60),
      updated_at: ago(55),
    },
  ],
}

// Parent chain: incident-response waiting on sub-chain at step "escalate"
const INCIDENT_CHAIN_DETAIL = {
  chain_id: 'chn_a1b2c3d4e5f6',
  chain_name: 'incident-response',
  status: 'waiting_sub_chain',
  current_step: 2,
  total_steps: 4,
  started_at: ago(12),
  updated_at: ago(1),
  execution_path: ['triage', 'escalate'],
  child_chain_ids: ['chn_sub_esc_001'],
  steps: [
    {
      name: 'triage',
      provider: 'webhook',
      status: 'completed',
      response_body: { priority: 'critical', alert_id: 'ALT-2847', service: 'api-gateway' },
      completed_at: ago(10),
    },
    {
      name: 'escalate',
      provider: 'pagerduty',
      status: 'waiting_sub_chain',
      sub_chain: 'escalation-runbook',
      child_chain_id: 'chn_sub_esc_001',
    },
    { name: 'notify-team', provider: 'slack', status: 'pending' },
    { name: 'auto-resolve', provider: 'webhook', status: 'pending' },
  ],
}

// Child sub-chain: escalation-runbook (running, spawned by incident-response)
const ESCALATION_CHAIN_DETAIL = {
  chain_id: 'chn_sub_esc_001',
  chain_name: 'escalation-runbook',
  status: 'running',
  current_step: 1,
  total_steps: 3,
  started_at: ago(1),
  updated_at: ago(0),
  parent_chain_id: 'chn_a1b2c3d4e5f6',
  execution_path: ['page-oncall'],
  steps: [
    {
      name: 'page-oncall',
      provider: 'pagerduty',
      status: 'completed',
      response_body: { responder: 'eng-oncall-primary', acknowledged: true },
      completed_at: ago(0.5),
    },
    { name: 'create-war-room', provider: 'slack', status: 'running' },
    { name: 'update-status-page', provider: 'webhook', status: 'pending' },
  ],
}

// DAG for incident-response (parent) with expanded sub-chain
const INCIDENT_DAG = {
  chain_name: 'incident-response',
  chain_id: 'chn_a1b2c3d4e5f6',
  status: 'waiting_sub_chain',
  nodes: [
    { name: 'triage', node_type: 'step', provider: 'webhook', action_type: 'triage_alert', status: 'completed', parallel_children: null, parallel_join: null },
    {
      name: 'escalate',
      node_type: 'sub_chain',
      provider: 'pagerduty',
      action_type: 'escalate',
      status: 'waiting_sub_chain',
      sub_chain_name: 'escalation-runbook',
      child_chain_id: 'chn_sub_esc_001',
      children: {
        chain_name: 'escalation-runbook',
        chain_id: 'chn_sub_esc_001',
        status: 'running',
        nodes: [
          { name: 'page-oncall', node_type: 'step', provider: 'pagerduty', action_type: 'page', status: 'completed', parallel_children: null, parallel_join: null },
          { name: 'create-war-room', node_type: 'step', provider: 'slack', action_type: 'create_channel', status: 'running', parallel_children: null, parallel_join: null },
          { name: 'update-status-page', node_type: 'step', provider: 'webhook', action_type: 'update_status', status: 'pending', parallel_children: null, parallel_join: null },
        ],
        edges: [
          { source: 'page-oncall', target: 'create-war-room', label: null, on_execution_path: true },
          { source: 'create-war-room', target: 'update-status-page', label: null, on_execution_path: false },
        ],
        execution_path: ['page-oncall', 'create-war-room'],
      },
      parallel_children: null,
      parallel_join: null,
    },
    { name: 'notify-team', node_type: 'step', provider: 'slack', action_type: 'send_message', status: 'pending', parallel_children: null, parallel_join: null },
    { name: 'auto-resolve', node_type: 'step', provider: 'webhook', action_type: 'resolve', status: 'pending', parallel_children: null, parallel_join: null },
  ],
  edges: [
    { source: 'triage', target: 'escalate', label: 'priority = critical', on_execution_path: true },
    { source: 'triage', target: 'notify-team', label: 'default', on_execution_path: false },
    { source: 'triage', target: 'auto-resolve', label: 'priority = low', on_execution_path: false },
    { source: 'escalate', target: 'notify-team', label: null, on_execution_path: false },
  ],
  execution_path: ['triage', 'escalate'],
}

// DAG for escalation-runbook sub-chain (standalone view)
const ESCALATION_DAG = {
  chain_name: 'escalation-runbook',
  chain_id: 'chn_sub_esc_001',
  status: 'running',
  nodes: [
    { name: 'page-oncall', node_type: 'step', provider: 'pagerduty', action_type: 'page', status: 'completed', parallel_children: null, parallel_join: null },
    { name: 'create-war-room', node_type: 'step', provider: 'slack', action_type: 'create_channel', status: 'running', parallel_children: null, parallel_join: null },
    { name: 'update-status-page', node_type: 'step', provider: 'webhook', action_type: 'update_status', status: 'pending', parallel_children: null, parallel_join: null },
  ],
  edges: [
    { source: 'page-oncall', target: 'create-war-room', label: null, on_execution_path: true },
    { source: 'create-war-room', target: 'update-status-page', label: null, on_execution_path: false },
  ],
  execution_path: ['page-oncall', 'create-war-room'],
}

// Completed deploy-pipeline with parallel step
const DEPLOY_CHAIN_DETAIL = {
  chain_id: 'chn_f7e8d9c0b1a2',
  chain_name: 'deploy-pipeline',
  status: 'completed',
  current_step: 5,
  total_steps: 5,
  started_at: ago(45),
  updated_at: ago(30),
  execution_path: ['validate', 'run-tests', 'deploy', 'notify-success'],
  steps: [
    { name: 'validate', provider: 'webhook', status: 'completed', response_body: { status: 'passed', build_id: 'build-4821' }, completed_at: ago(44) },
    { name: 'run-tests', provider: 'webhook', status: 'completed', response_body: { passed: 142, failed: 0 }, completed_at: ago(40) },
    {
      name: 'deploy',
      provider: 'webhook',
      status: 'completed',
      completed_at: ago(32),
      parallel_sub_steps: [
        { name: 'deploy-us-east', provider: 'webhook', status: 'completed', response_body: { region: 'us-east-1', instances: 3 }, completed_at: ago(33) },
        { name: 'deploy-eu-west', provider: 'webhook', status: 'completed', response_body: { region: 'eu-west-1', instances: 2 }, completed_at: ago(32) },
        { name: 'deploy-ap-south', provider: 'webhook', status: 'completed', response_body: { region: 'ap-south-1', instances: 2 }, completed_at: ago(34) },
      ],
    },
    { name: 'notify-success', provider: 'slack', status: 'completed', completed_at: ago(30) },
    { name: 'notify-failure', provider: 'pagerduty', status: 'skipped' },
  ],
}

const DEPLOY_DAG = {
  chain_name: 'deploy-pipeline',
  chain_id: 'chn_f7e8d9c0b1a2',
  status: 'completed',
  nodes: [
    { name: 'validate', node_type: 'step', provider: 'webhook', action_type: 'validate_build', status: 'completed', parallel_children: null, parallel_join: null },
    { name: 'run-tests', node_type: 'step', provider: 'webhook', action_type: 'run_tests', status: 'completed', parallel_children: null, parallel_join: null },
    {
      name: 'deploy',
      node_type: 'parallel',
      provider: null,
      action_type: null,
      status: 'completed',
      parallel_children: [
        { name: 'deploy-us-east', node_type: 'step', provider: 'webhook', action_type: 'deploy_region', status: 'completed', parallel_children: null, parallel_join: null },
        { name: 'deploy-eu-west', node_type: 'step', provider: 'webhook', action_type: 'deploy_region', status: 'completed', parallel_children: null, parallel_join: null },
        { name: 'deploy-ap-south', node_type: 'step', provider: 'webhook', action_type: 'deploy_region', status: 'completed', parallel_children: null, parallel_join: null },
      ],
      parallel_join: 'All',
    },
    { name: 'notify-success', node_type: 'step', provider: 'slack', action_type: 'send_message', status: 'completed', parallel_children: null, parallel_join: null },
    { name: 'notify-failure', node_type: 'step', provider: 'pagerduty', action_type: 'create_incident', status: 'skipped', parallel_children: null, parallel_join: null },
  ],
  edges: [
    { source: 'validate', target: 'run-tests', label: 'status = passed', on_execution_path: true },
    { source: 'validate', target: 'notify-failure', label: 'status = failed', on_execution_path: false },
    { source: 'run-tests', target: 'deploy', label: null, on_execution_path: true },
    { source: 'deploy', target: 'notify-success', label: null, on_execution_path: true },
  ],
  execution_path: ['validate', 'run-tests', 'deploy', 'notify-success'],
}

// ---------------------------------------------------------------------------
// Route mocking helper
// ---------------------------------------------------------------------------

function mockSubChainsApi(page: import('@playwright/test').Page) {
  return page.route('**/v1/**', async (route) => {
    const url = route.request().url()

    // Chain DAG by chain_id
    const dagMatch = url.match(/\/v1\/chains\/([^/]+)\/dag/)
    if (dagMatch && !url.includes('definitions')) {
      const id = dagMatch[1]
      const dag =
        id === 'chn_a1b2c3d4e5f6' ? INCIDENT_DAG
        : id === 'chn_sub_esc_001' ? ESCALATION_DAG
        : id === 'chn_f7e8d9c0b1a2' ? DEPLOY_DAG
        : null
      await route.fulfill({
        status: dag ? 200 : 404,
        contentType: 'application/json',
        body: JSON.stringify(dag ?? { error: 'not found' }),
      })
      return
    }

    // Chain detail by chain_id (match before query string)
    const detailMatch = url.match(/\/v1\/chains\/([^/?]+)(?:\?|$)/)
    if (detailMatch && !url.includes('/dag') && !url.includes('/cancel') && !url.includes('/definitions')) {
      const id = detailMatch[1]
      const detail =
        id === 'chn_a1b2c3d4e5f6' ? INCIDENT_CHAIN_DETAIL
        : id === 'chn_sub_esc_001' ? ESCALATION_CHAIN_DETAIL
        : id === 'chn_f7e8d9c0b1a2' ? DEPLOY_CHAIN_DETAIL
        : null
      await route.fulfill({
        status: detail ? 200 : 404,
        contentType: 'application/json',
        body: JSON.stringify(detail ?? { error: 'not found' }),
      })
      return
    }

    // Chain list
    if (url.match(/\/v1\/chains(\?|$)/)) {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(CHAINS_LIST),
      })
      return
    }

    // Default: empty response
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify([]),
    })
  })
}

// ---------------------------------------------------------------------------
// Tests (each produces one screenshot)
// ---------------------------------------------------------------------------

test.describe('Sub-Chain Screenshots', () => {
  test('sub-chains-list — Chains list with sub-chain statuses', async ({ page }) => {
    await mockSubChainsApi(page)
    await page.goto(`${BASE_URL}/chains?namespace=ops&tenant=acme-corp`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'sub-chains-list.png'),
      fullPage: true,
    })
  })

  test('sub-chains-detail — Parent chain waiting on sub-chain with expanded DAG', async ({ page }) => {
    await mockSubChainsApi(page)
    await page.goto(
      `${BASE_URL}/chains/chn_a1b2c3d4e5f6?namespace=ops&tenant=acme-corp&expand=all`,
      { waitUntil: 'networkidle' },
    )
    await page.waitForTimeout(800)

    await page.screenshot({
      path: path.join(ASSETS, 'sub-chains-detail.png'),
      fullPage: true,
    })
  })

  test('sub-chains-step-detail — Sub-chain step detail with child chain link', async ({ page }) => {
    await mockSubChainsApi(page)
    await page.goto(
      `${BASE_URL}/chains/chn_a1b2c3d4e5f6?namespace=ops&tenant=acme-corp&expand=all`,
      { waitUntil: 'networkidle' },
    )
    await page.waitForTimeout(1000)

    // Click the "escalate" node in the React Flow canvas
    const escalateNode = page.locator('.react-flow__node', { hasText: 'escalate' }).first()
    if (await escalateNode.isVisible({ timeout: 3000 }).catch(() => false)) {
      await escalateNode.click()
      await page.waitForTimeout(300)
    }

    await page.screenshot({
      path: path.join(ASSETS, 'sub-chains-step-detail.png'),
      fullPage: true,
    })
  })

  test('sub-chains-child-execution — Sub-chain execution with parent link', async ({ page }) => {
    await mockSubChainsApi(page)
    await page.goto(
      `${BASE_URL}/chains/chn_sub_esc_001?namespace=ops&tenant=acme-corp`,
      { waitUntil: 'networkidle' },
    )
    await page.waitForTimeout(800)

    await page.screenshot({
      path: path.join(ASSETS, 'sub-chains-child-execution.png'),
      fullPage: true,
    })
  })

  test('sub-chains-dag-api — Incident-response DAG JSON (pretty)', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 })
    const prettyJson = JSON.stringify(INCIDENT_DAG, null, 2)

    await page.setContent(`
      <html>
        <head>
          <style>
            body {
              background: #1e1e2e;
              color: #cdd6f4;
              font-family: 'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace;
              font-size: 12px;
              padding: 24px;
              margin: 0;
              line-height: 1.5;
            }
            .header {
              color: #89b4fa;
              font-size: 14px;
              font-weight: bold;
              margin-bottom: 16px;
              padding-bottom: 8px;
              border-bottom: 1px solid #45475a;
            }
            pre { white-space: pre-wrap; word-break: break-word; }
            .string { color: #a6e3a1; }
            .number { color: #fab387; }
            .boolean { color: #f38ba8; }
            .null { color: #6c7086; }
            .key { color: #89b4fa; }
          </style>
        </head>
        <body>
          <div class="header">GET /v1/chains/chn_a1b2c3d4e5f6/dag — incident-response with expanded sub-chain</div>
          <pre>${syntaxHighlight(prettyJson)}</pre>
        </body>
      </html>
    `)
    await page.waitForTimeout(300)

    await page.screenshot({
      path: path.join(ASSETS, 'sub-chains-dag-api.png'),
      fullPage: false,
    })
  })

  test('sub-chains-deploy-pipeline — Completed chain with parallel steps', async ({ page }) => {
    await mockSubChainsApi(page)
    await page.goto(
      `${BASE_URL}/chains/chn_f7e8d9c0b1a2?namespace=ops&tenant=acme-corp`,
      { waitUntil: 'networkidle' },
    )
    await page.waitForTimeout(1000)

    // Click the "deploy" parallel node in the React Flow canvas
    const deployNode = page.locator('.react-flow__node', { hasText: 'deploy' }).first()
    if (await deployNode.isVisible({ timeout: 3000 }).catch(() => false)) {
      await deployNode.click()
      await page.waitForTimeout(300)
    }

    await page.screenshot({
      path: path.join(ASSETS, 'sub-chains-deploy-pipeline.png'),
      fullPage: true,
    })
  })

  test('sub-chains-rules — Rules page', async ({ page }) => {
    await page.route('**/v1/**', async (route) => {
      const url = route.request().url()

      if (url.match(/\/v1\/rules(\?|$)/)) {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify([
            {
              name: 'incident-detected → incident-response',
              priority: 10,
              description: 'Triggers incident-response chain on critical severity alerts',
              enabled: true,
            },
            {
              name: 'deploy-requested → deploy-pipeline',
              priority: 5,
              description: 'Triggers deploy-pipeline chain on deploy requests (with parallel regions)',
              enabled: true,
            },
            {
              name: 'user-signup → user-onboarding',
              priority: 1,
              description: 'Triggers user-onboarding chain for new user signups',
              enabled: true,
            },
            {
              name: 'data-sync-schedule → data-sync',
              priority: 3,
              description: 'Triggers data-sync chain with parallel + sub-chain steps',
              enabled: false,
            },
          ]),
        })
        return
      }

      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([]),
      })
    })

    await page.goto(`${BASE_URL}/rules`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'sub-chains-rules.png'),
      fullPage: true,
    })
  })
})

/** Simple JSON syntax highlighter for screenshot. */
function syntaxHighlight(json: string): string {
  return json
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(
      /("(\\u[\da-fA-F]{4}|\\[^u]|[^\\"])*"(\s*:)?|\b(true|false|null)\b|-?\d+(?:\.\d*)?(?:[eE][+-]?\d+)?)/g,
      (match) => {
        let cls = 'number'
        if (/^"/.test(match)) {
          cls = /:$/.test(match) ? 'key' : 'string'
        } else if (/true|false/.test(match)) {
          cls = 'boolean'
        } else if (/null/.test(match)) {
          cls = 'null'
        }
        return `<span class="${cls}">${match}</span>`
      },
    )
}
