/**
 * Playwright script to capture Rule Playground screenshots with realistic mock data.
 *
 * Usage: npx playwright test playground-screenshots --project chromium
 */
import { test } from '@playwright/test'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)
const ASSETS = path.resolve(__dirname, '../../docs/book/admin-ui/assets')

// ---------------------------------------------------------------------------
// Mock API responses with realistic data
// ---------------------------------------------------------------------------

const SUPPRESS_RESPONSE = {
  verdict: 'suppress',
  matched_rule: 'block-spam-emails',
  has_errors: false,
  total_rules_evaluated: 3,
  total_rules_skipped: 0,
  evaluation_duration_us: 142,
  trace: [
    {
      rule_name: 'block-spam-emails',
      priority: 1,
      enabled: true,
      condition_display: 'action.payload.category == "spam"',
      result: 'matched',
      evaluation_duration_us: 38,
      action: 'suppress',
      source: 'yaml',
      description: 'Block emails flagged as spam by the content filter',
      skip_reason: null,
      error: null,
    },
    {
      rule_name: 'reroute-urgent-alerts',
      priority: 5,
      enabled: true,
      condition_display: 'action.payload.priority == "urgent"',
      result: 'not_matched',
      evaluation_duration_us: 22,
      action: 'reroute',
      source: 'yaml',
      description: 'Send urgent alerts via SMS instead of email',
      skip_reason: 'Evaluation stopped after first match',
      error: null,
    },
    {
      rule_name: 'enrich-email-payload',
      priority: 10,
      enabled: true,
      condition_display: 'action.action_type == "email"',
      result: 'not_matched',
      evaluation_duration_us: 18,
      action: 'modify',
      source: 'yaml',
      description: 'Add default sender and reply-to headers to outgoing emails',
      skip_reason: 'Evaluation stopped after first match',
      error: null,
    },
  ],
  context: {
    time: {
      hour: 14,
      minute: 32,
      second: 8,
      day: 12,
      month: 2,
      year: 2026,
      weekday: 'Thursday',
      weekday_num: 4,
      timestamp: 1771080728,
    },
    environment_keys: ['HOME', 'PATH', 'LANG', 'SHELL', 'USER', 'HOSTNAME', 'RUST_LOG'],
    effective_timezone: null,
  },
  modified_payload: null,
}

const EVALUATE_ALL_RESPONSE = {
  verdict: 'suppress',
  matched_rule: 'block-spam-emails',
  has_errors: false,
  total_rules_evaluated: 5,
  total_rules_skipped: 1,
  evaluation_duration_us: 287,
  trace: [
    {
      rule_name: 'block-spam-emails',
      priority: 1,
      enabled: true,
      condition_display: 'action.payload.category == "spam"',
      result: 'matched',
      evaluation_duration_us: 41,
      action: 'suppress',
      source: 'yaml',
      description: 'Block emails flagged as spam by the content filter',
      skip_reason: null,
      error: null,
    },
    {
      rule_name: 'rate-limit-alerts',
      priority: 3,
      enabled: true,
      condition_display: 'action.action_type == "alert" && state.get("alert_count") > "10"',
      result: 'not_matched',
      evaluation_duration_us: 63,
      action: 'throttle',
      source: 'cel',
      description: 'Throttle alert actions when rate exceeds 10 per window',
      skip_reason: null,
      error: null,
    },
    {
      rule_name: 'reroute-urgent-alerts',
      priority: 5,
      enabled: true,
      condition_display: 'action.payload.priority == "urgent"',
      result: 'not_matched',
      evaluation_duration_us: 22,
      action: 'reroute',
      source: 'yaml',
      description: 'Send urgent alerts via SMS instead of email',
      skip_reason: null,
      error: null,
    },
    {
      rule_name: 'weekend-maintenance',
      priority: 7,
      enabled: false,
      condition_display: 'time.weekday_num >= 6',
      result: 'skipped',
      evaluation_duration_us: 0,
      action: 'suppress',
      source: 'yaml',
      description: 'Suppress non-critical actions during weekend maintenance',
      skip_reason: 'Rule is disabled',
      error: null,
    },
    {
      rule_name: 'enrich-email-payload',
      priority: 10,
      enabled: true,
      condition_display: 'action.action_type == "email"',
      result: 'not_matched',
      evaluation_duration_us: 19,
      action: 'modify',
      source: 'yaml',
      description: 'Add default sender and reply-to headers to outgoing emails',
      skip_reason: null,
      error: null,
    },
    {
      rule_name: 'log-all-actions',
      priority: 100,
      enabled: true,
      condition_display: 'true',
      result: 'matched',
      evaluation_duration_us: 12,
      action: 'allow',
      source: 'cel',
      description: 'Catch-all: allow and log any action that reaches this point',
      skip_reason: null,
      error: null,
    },
  ],
  context: {
    time: {
      hour: 14,
      minute: 32,
      second: 45,
      day: 12,
      month: 2,
      year: 2026,
      weekday: 'Thursday',
      weekday_num: 4,
      timestamp: 1771080765,
    },
    environment_keys: ['HOME', 'PATH', 'LANG', 'SHELL', 'USER', 'HOSTNAME', 'RUST_LOG'],
    effective_timezone: null,
  },
  modified_payload: null,
}

const MODIFY_RESPONSE = {
  verdict: 'modify',
  matched_rule: 'enrich-email-payload',
  has_errors: false,
  total_rules_evaluated: 3,
  total_rules_skipped: 0,
  evaluation_duration_us: 98,
  trace: [
    {
      rule_name: 'block-spam-emails',
      priority: 1,
      enabled: true,
      condition_display: 'action.payload.category == "spam"',
      result: 'not_matched',
      evaluation_duration_us: 31,
      action: 'suppress',
      source: 'yaml',
      description: 'Block emails flagged as spam by the content filter',
      skip_reason: null,
      error: null,
    },
    {
      rule_name: 'reroute-urgent-alerts',
      priority: 5,
      enabled: true,
      condition_display: 'action.payload.priority == "urgent"',
      result: 'not_matched',
      evaluation_duration_us: 20,
      action: 'reroute',
      source: 'yaml',
      description: 'Send urgent alerts via SMS instead of email',
      skip_reason: null,
      error: null,
    },
    {
      rule_name: 'enrich-email-payload',
      priority: 10,
      enabled: true,
      condition_display: 'action.action_type == "email"',
      result: 'matched',
      evaluation_duration_us: 47,
      action: 'modify',
      source: 'yaml',
      description: 'Add default sender and reply-to headers to outgoing emails',
      skip_reason: null,
      error: null,
    },
  ],
  context: {
    time: {
      hour: 14,
      minute: 33,
      second: 12,
      day: 12,
      month: 2,
      year: 2026,
      weekday: 'Thursday',
      weekday_num: 4,
      timestamp: 1771080792,
    },
    environment_keys: ['HOME', 'PATH', 'LANG', 'SHELL', 'USER', 'HOSTNAME', 'RUST_LOG'],
    effective_timezone: null,
  },
  modified_payload: {
    to: 'user@example.com',
    subject: 'Order Confirmation #12847',
    from: 'noreply@acme.com',
    reply_to: 'support@acme.com',
    headers: {
      'X-Mailer': 'Acteon/1.0',
      'X-Priority': '3',
    },
  },
}

// ---------------------------------------------------------------------------
// Helper: set up route mocking for a specific evaluate response
// ---------------------------------------------------------------------------

function mockApi(page: import('@playwright/test').Page, evaluateResponse: object) {
  // Single route handler for all /v1/ API calls
  return page.route('**/v1/**', async (route) => {
    const url = route.request().url()
    if (url.includes('/v1/rules/evaluate')) {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(evaluateResponse),
      })
    } else {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([]),
      })
    }
  })
}

// ---------------------------------------------------------------------------
// Helper: fill the form fields
// ---------------------------------------------------------------------------

async function fillForm(
  page: import('@playwright/test').Page,
  opts: {
    ns: string
    tenant: string
    provider: string
    actionType: string
    payload: object
  },
) {
  const nsInput = page.getByLabel('Namespace *')
  await nsInput.clear()
  await nsInput.fill(opts.ns)

  const tenantInput = page.getByLabel('Tenant *')
  await tenantInput.clear()
  await tenantInput.fill(opts.tenant)

  const providerInput = page.getByLabel('Provider *')
  await providerInput.clear()
  await providerInput.fill(opts.provider)

  const actionTypeInput = page.getByLabel('Action Type *')
  await actionTypeInput.clear()
  await actionTypeInput.fill(opts.actionType)

  const payloadArea = page.locator('textarea').first()
  await payloadArea.fill(JSON.stringify(opts.payload, null, 2))
}

// ---------------------------------------------------------------------------
// Tests (each produces one screenshot)
// ---------------------------------------------------------------------------

// Use production build for reliable CSS rendering
const BASE_URL = 'http://localhost:4173'

test.use({ viewport: { width: 1440, height: 900 } })

test.describe('Rule Playground Screenshots', () => {
  test('playground with suppress verdict', async ({ page }) => {
    await mockApi(page, SUPPRESS_RESPONSE)
    await page.goto(`${BASE_URL}/playground`, { waitUntil: 'networkidle' })

    await fillForm(page, {
      ns: 'notifications',
      tenant: 'acme-corp',
      provider: 'email',
      actionType: 'alert',
      payload: {
        category: 'spam',
        to: 'user@example.com',
        subject: 'You won a prize!',
        body: 'Click here to claim your reward...',
      },
    })

    // Click Evaluate and wait for results
    await page.getByRole('button', { name: /Evaluate/i }).click()
    await page.locator('table').waitFor({ state: 'visible', timeout: 5000 })
    await page.waitForTimeout(300)

    // Expand the first trace row to show details
    await page.locator('table tbody tr').first().click()
    await page.waitForTimeout(200)

    // Open Context section
    await page.getByText('Context').click()
    await page.waitForTimeout(200)

    await page.screenshot({
      path: path.join(ASSETS, 'playground.png'),
      fullPage: true,
    })
  })

  test('playground with evaluate-all and disabled rules', async ({ page }) => {
    await mockApi(page, EVALUATE_ALL_RESPONSE)
    await page.goto(`${BASE_URL}/playground`, { waitUntil: 'networkidle' })

    await fillForm(page, {
      ns: 'notifications',
      tenant: 'acme-corp',
      provider: 'email',
      actionType: 'alert',
      payload: {
        category: 'spam',
        to: 'ops-team@acme.com',
        subject: 'Server CPU alert',
        priority: 'normal',
      },
    })

    // Enable both toggles
    await page.getByText('Include Disabled Rules').click()
    await page.getByText('Evaluate All Rules').click()

    // Click Evaluate and wait for results
    await page.getByRole('button', { name: /Evaluate/i }).click()
    await page.locator('table').waitFor({ state: 'visible', timeout: 5000 })
    await page.waitForTimeout(300)

    await page.screenshot({
      path: path.join(ASSETS, 'playground-evaluate-all.png'),
      fullPage: true,
    })
  })

  test('playground with modify verdict showing payload diff', async ({ page }) => {
    await mockApi(page, MODIFY_RESPONSE)
    await page.goto(`${BASE_URL}/playground`, { waitUntil: 'networkidle' })

    await fillForm(page, {
      ns: 'notifications',
      tenant: 'acme-corp',
      provider: 'email',
      actionType: 'email',
      payload: {
        to: 'user@example.com',
        subject: 'Order Confirmation #12847',
      },
    })

    // Click Evaluate and wait for results
    await page.getByRole('button', { name: /Evaluate/i }).click()
    await page.locator('table').waitFor({ state: 'visible', timeout: 5000 })
    await page.waitForTimeout(300)

    await page.screenshot({
      path: path.join(ASSETS, 'playground-modify.png'),
      fullPage: true,
    })
  })
})
