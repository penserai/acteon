/**
 * Playwright script to capture Chain Definitions admin UI screenshots with realistic mock data.
 *
 * Usage: npx playwright test chain-definition-screenshots --project chromium
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
// Mock data — Chain Definitions
// ---------------------------------------------------------------------------

const DEFINITIONS_LIST = {
  definitions: [
    {
      name: 'deploy-pipeline',
      steps_count: 5,
      has_branches: true,
      has_parallel: true,
      has_sub_chains: false,
      on_failure: 'Abort',
      timeout_seconds: 600,
    },
    {
      name: 'incident-response',
      steps_count: 4,
      has_branches: true,
      has_parallel: false,
      has_sub_chains: true,
      on_failure: 'Abort',
      timeout_seconds: 300,
    },
    {
      name: 'user-onboarding',
      steps_count: 3,
      has_branches: false,
      has_parallel: false,
      has_sub_chains: false,
      on_failure: 'AbortNoDlq',
      timeout_seconds: null,
    },
    {
      name: 'data-sync',
      steps_count: 6,
      has_branches: false,
      has_parallel: true,
      has_sub_chains: true,
      on_failure: 'Abort',
      timeout_seconds: 900,
    },
  ],
}

const DEPLOY_PIPELINE_DEFINITION = {
  name: 'deploy-pipeline',
  steps: [
    {
      name: 'validate',
      provider: 'webhook',
      action_type: 'validate_build',
      payload_template: { build_id: '{{action.build_id}}', environment: '{{action.environment}}' },
      branches: [
        { field: 'body.status', operator: 'Eq', value: 'passed', target: 'run-tests' },
        { field: 'body.status', operator: 'Eq', value: 'failed', target: 'notify-failure' },
      ],
      default_next: 'notify-failure',
    },
    {
      name: 'run-tests',
      provider: 'webhook',
      action_type: 'run_tests',
      payload_template: { suite: 'integration', build_id: '{{steps.validate.body.build_id}}' },
      branches: [],
    },
    {
      name: 'deploy',
      provider: 'webhook',
      action_type: 'deploy',
      payload_template: { service: '{{action.service}}', version: '{{action.version}}' },
      branches: [],
      parallel: {
        steps: [
          { name: 'deploy-us-east', provider: 'webhook', action_type: 'deploy_region', payload_template: { region: 'us-east-1' }, branches: [] },
          { name: 'deploy-eu-west', provider: 'webhook', action_type: 'deploy_region', payload_template: { region: 'eu-west-1' }, branches: [] },
          { name: 'deploy-ap-south', provider: 'webhook', action_type: 'deploy_region', payload_template: { region: 'ap-south-1' }, branches: [] },
        ],
        join: 'All',
        on_failure: 'FailFast',
        timeout_seconds: 120,
        max_concurrency: 2,
      },
    },
    {
      name: 'notify-success',
      provider: 'slack',
      action_type: 'send_message',
      payload_template: { channel: '#deploys', text: 'Deployment successful for {{action.service}}' },
      branches: [],
    },
    {
      name: 'notify-failure',
      provider: 'pagerduty',
      action_type: 'create_incident',
      payload_template: { severity: 'high', title: 'Deploy failed: {{action.service}}' },
      branches: [],
    },
  ],
  on_failure: 'Abort',
  timeout_seconds: 600,
  on_cancel: { provider: 'slack', action_type: 'send_message' },
}

const INCIDENT_RESPONSE_DEFINITION = {
  name: 'incident-response',
  steps: [
    {
      name: 'triage',
      provider: 'webhook',
      action_type: 'triage_alert',
      payload_template: { alert_id: '{{action.alert_id}}', severity: '{{action.severity}}' },
      branches: [
        { field: 'body.priority', operator: 'Eq', value: 'critical', target: 'escalate' },
        { field: 'body.priority', operator: 'Eq', value: 'low', target: 'auto-resolve' },
      ],
      default_next: 'notify-team',
    },
    {
      name: 'escalate',
      provider: 'pagerduty',
      action_type: 'escalate',
      payload_template: { urgency: 'high' },
      branches: [],
      sub_chain: 'escalation-runbook',
    },
    {
      name: 'notify-team',
      provider: 'slack',
      action_type: 'send_message',
      payload_template: { channel: '#incidents' },
      branches: [],
    },
    {
      name: 'auto-resolve',
      provider: 'webhook',
      action_type: 'resolve',
      payload_template: { auto: true },
      branches: [],
    },
  ],
  on_failure: 'Abort',
  timeout_seconds: 300,
}

const DEPLOY_PIPELINE_DAG = {
  chain_name: 'deploy-pipeline',
  chain_id: null,
  status: null,
  nodes: [
    { name: 'validate', node_type: 'step', provider: 'webhook', action_type: 'validate_build', status: null, parallel_children: null, parallel_join: null },
    { name: 'run-tests', node_type: 'step', provider: 'webhook', action_type: 'run_tests', status: null, parallel_children: null, parallel_join: null },
    {
      name: 'deploy',
      node_type: 'parallel',
      provider: null,
      action_type: null,
      status: null,
      parallel_children: [
        { name: 'deploy-us-east', node_type: 'step', provider: 'webhook', action_type: 'deploy_region', status: null, parallel_children: null, parallel_join: null },
        { name: 'deploy-eu-west', node_type: 'step', provider: 'webhook', action_type: 'deploy_region', status: null, parallel_children: null, parallel_join: null },
        { name: 'deploy-ap-south', node_type: 'step', provider: 'webhook', action_type: 'deploy_region', status: null, parallel_children: null, parallel_join: null },
      ],
      parallel_join: 'All',
    },
    { name: 'notify-success', node_type: 'step', provider: 'slack', action_type: 'send_message', status: null, parallel_children: null, parallel_join: null },
    { name: 'notify-failure', node_type: 'step', provider: 'pagerduty', action_type: 'create_incident', status: null, parallel_children: null, parallel_join: null },
  ],
  edges: [
    { source: 'validate', target: 'run-tests', label: 'status = passed', on_execution_path: false },
    { source: 'validate', target: 'notify-failure', label: 'status = failed', on_execution_path: false },
    { source: 'run-tests', target: 'deploy', label: null, on_execution_path: false },
    { source: 'deploy', target: 'notify-success', label: null, on_execution_path: false },
  ],
  execution_path: [],
}

// ---------------------------------------------------------------------------
// Route mocking helper
// ---------------------------------------------------------------------------

function mockChainDefinitionsApi(page: import('@playwright/test').Page) {
  return page.route('**/v1/**', async (route) => {
    const url = route.request().url()
    const method = route.request().method()

    // Chain definition DAG
    if (url.includes('/v1/chains/definitions/') && url.endsWith('/dag')) {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(DEPLOY_PIPELINE_DAG),
      })
      return
    }

    // Single chain definition by name
    const nameMatch = url.match(/\/v1\/chains\/definitions\/([^/?]+)$/)
    if (nameMatch && method === 'GET') {
      const name = nameMatch[1]
      const def = name === 'deploy-pipeline'
        ? DEPLOY_PIPELINE_DEFINITION
        : name === 'incident-response'
          ? INCIDENT_RESPONSE_DEFINITION
          : null
      await route.fulfill({
        status: def ? 200 : 404,
        contentType: 'application/json',
        body: JSON.stringify(def ?? { error: 'not found' }),
      })
      return
    }

    // PUT chain definition (save)
    if (nameMatch && method === 'PUT') {
      const body = route.request().postDataJSON()
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(body),
      })
      return
    }

    // List chain definitions
    if (url.match(/\/v1\/chains\/definitions(\?|$)/)) {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(DEFINITIONS_LIST),
      })
      return
    }

    // Default: empty response for any other API call
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

test.describe('Chain Definition Screenshots', () => {
  test('chain-definitions-list — List page with populated data', async ({ page }) => {
    await mockChainDefinitionsApi(page)
    await page.goto(`${BASE_URL}/chain-definitions`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'chain-definitions-list.png'),
      fullPage: true,
    })
  })

  test('chain-definitions-detail — Detail drawer with overview, steps, and DAG', async ({
    page,
  }) => {
    await mockChainDefinitionsApi(page)
    await page.goto(`${BASE_URL}/chain-definitions`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Click the deploy-pipeline row to open detail drawer
    await page.getByRole('row', { name: /deploy-pipeline/ }).click()
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'chain-definitions-detail.png'),
      fullPage: true,
    })
  })

  test('chain-definitions-detail-steps — Steps tab in detail drawer', async ({ page }) => {
    await mockChainDefinitionsApi(page)
    await page.goto(`${BASE_URL}/chain-definitions`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Open detail drawer for deploy-pipeline
    await page.getByRole('row', { name: /deploy-pipeline/ }).click()
    await page.waitForTimeout(500)

    // Switch to the Steps tab
    await page.getByRole('tab', { name: 'Steps' }).click()
    await page.waitForTimeout(300)

    await page.screenshot({
      path: path.join(ASSETS, 'chain-definitions-detail-steps.png'),
      fullPage: true,
    })
  })

  test('chain-definitions-detail-dag — DAG tab in detail drawer', async ({ page }) => {
    await mockChainDefinitionsApi(page)
    await page.goto(`${BASE_URL}/chain-definitions`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Open detail drawer for deploy-pipeline
    await page.getByRole('row', { name: /deploy-pipeline/ }).click()
    await page.waitForTimeout(500)

    // Switch to the DAG tab
    await page.getByRole('tab', { name: 'DAG' }).click()
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'chain-definitions-detail-dag.png'),
      fullPage: true,
    })
  })

  test('chain-definitions-create-modal — Create definition modal with filled form', async ({
    page,
  }) => {
    await mockChainDefinitionsApi(page)
    await page.goto(`${BASE_URL}/chain-definitions`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Open the Create Definition modal
    await page.getByRole('button', { name: 'Create Definition' }).click()
    await page.waitForTimeout(300)

    // Fill in chain metadata
    await page.getByLabel('Name').fill('order-processing')
    await page.getByLabel('Timeout (seconds)').fill('300')

    // Add first step — will be auto-created as Provider type
    await page.getByRole('button', { name: /Add Step/i }).click()
    await page.waitForTimeout(200)

    await page.screenshot({
      path: path.join(ASSETS, 'chain-definitions-create-modal.png'),
      fullPage: true,
    })
  })

  test('chain-definitions-edit-modal — Edit existing definition with steps', async ({ page }) => {
    await mockChainDefinitionsApi(page)
    await page.goto(`${BASE_URL}/chain-definitions`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Open detail drawer for deploy-pipeline
    await page.getByRole('row', { name: /deploy-pipeline/ }).click()
    await page.waitForTimeout(500)

    // Click the Edit button in the drawer
    await page.getByRole('button', { name: 'Edit' }).click()
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'chain-definitions-edit-modal.png'),
      fullPage: true,
    })
  })

  test('chain-definitions-json-tab — JSON editor tab', async ({ page }) => {
    await mockChainDefinitionsApi(page)
    await page.goto(`${BASE_URL}/chain-definitions`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Open detail drawer for deploy-pipeline, then edit
    await page.getByRole('row', { name: /deploy-pipeline/ }).click()
    await page.waitForTimeout(500)
    await page.getByRole('button', { name: 'Edit' }).click()
    await page.waitForTimeout(500)

    // Switch to the JSON tab inside the modal
    await page.getByRole('tab', { name: 'JSON' }).click()
    await page.waitForTimeout(300)

    await page.screenshot({
      path: path.join(ASSETS, 'chain-definitions-json-editor.png'),
      fullPage: true,
    })
  })

  test('chain-definitions-delete — Delete confirmation modal', async ({ page }) => {
    await mockChainDefinitionsApi(page)
    await page.goto(`${BASE_URL}/chain-definitions`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Click the Delete button on the first row
    await page.getByRole('button', { name: 'Delete' }).first().click()
    await page.waitForTimeout(300)

    await page.screenshot({
      path: path.join(ASSETS, 'chain-definitions-delete-confirm.png'),
      fullPage: true,
    })
  })
})
