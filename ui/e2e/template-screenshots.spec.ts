/**
 * Playwright script to capture Templates admin UI screenshots with realistic mock data.
 *
 * Usage: npx playwright test template-screenshots --project chromium
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
// Mock data — Templates
// ---------------------------------------------------------------------------

const ALERT_BODY_CONTENT = `<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8" />
  <title>Alert Notification</title>
</head>
<body style="font-family: sans-serif; background: #f9fafb; padding: 24px;">
  <div style="max-width: 600px; margin: 0 auto; background: #fff; border-radius: 8px; padding: 32px; border: 1px solid #e5e7eb;">
    <h1 style="color: {% if severity == 'critical' %}#dc2626{% elif severity == 'warning' %}#d97706{% else %}#2563eb{% endif %}; margin-top: 0;">
      {{ severity | upper }}: {{ service }}
    </h1>
    <p style="color: #374151;">{{ message }}</p>
    {% if details %}
    <table style="width: 100%; border-collapse: collapse; margin-top: 16px;">
      {% for key, value in details.items() %}
      <tr>
        <td style="padding: 8px; border: 1px solid #e5e7eb; font-weight: 600; background: #f9fafb;">{{ key }}</td>
        <td style="padding: 8px; border: 1px solid #e5e7eb;">{{ value }}</td>
      </tr>
      {% endfor %}
    </table>
    {% endif %}
    <p style="color: #6b7280; font-size: 12px; margin-top: 24px; border-top: 1px solid #e5e7eb; padding-top: 16px;">
      Generated at {{ timestamp }} &bull; Acteon Notification Gateway
    </p>
  </div>
</body>
</html>`

const ORDER_CONFIRMATION_CONTENT = `<!DOCTYPE html>
<html>
<head><meta charset="utf-8" /><title>Order Confirmation</title></head>
<body style="font-family: sans-serif; background: #f3f4f6; padding: 24px;">
  <div style="max-width: 640px; margin: 0 auto; background: #fff; border-radius: 8px; padding: 32px;">
    <img src="{{ logo_url }}" alt="{{ company_name }}" style="height: 40px; margin-bottom: 24px;" />
    <h1 style="color: #111827;">Thank you for your order, {{ customer_name }}!</h1>
    <p style="color: #6b7280;">Order <strong>#{{ order_id }}</strong> has been confirmed and is being prepared.</p>
    <table style="width: 100%; border-collapse: collapse; margin: 24px 0;">
      <thead>
        <tr style="background: #f9fafb;">
          <th style="text-align: left; padding: 12px; border-bottom: 2px solid #e5e7eb;">Item</th>
          <th style="text-align: right; padding: 12px; border-bottom: 2px solid #e5e7eb;">Qty</th>
          <th style="text-align: right; padding: 12px; border-bottom: 2px solid #e5e7eb;">Price</th>
        </tr>
      </thead>
      <tbody>
        {% for item in items %}
        <tr>
          <td style="padding: 12px; border-bottom: 1px solid #f3f4f6;">{{ item.name }}</td>
          <td style="padding: 12px; border-bottom: 1px solid #f3f4f6; text-align: right;">{{ item.quantity }}</td>
          <td style="padding: 12px; border-bottom: 1px solid #f3f4f6; text-align: right;">\${{ item.price }}</td>
        </tr>
        {% endfor %}
      </tbody>
      <tfoot>
        <tr>
          <td colspan="2" style="padding: 12px; text-align: right; font-weight: 600;">Total:</td>
          <td style="padding: 12px; text-align: right; font-weight: 700; color: #111827;">\${{ total }}</td>
        </tr>
      </tfoot>
    </table>
    <p style="color: #6b7280; font-size: 13px;">Estimated delivery: <strong>{{ delivery_date }}</strong></p>
    <hr style="border: none; border-top: 1px solid #e5e7eb; margin: 24px 0;" />
    <p style="color: #9ca3af; font-size: 12px;">&copy; {{ year }} {{ company_name }}. All rights reserved.</p>
  </div>
</body>
</html>`

const SLACK_NOTIFICATION_CONTENT = `{
  "blocks": [
    {
      "type": "header",
      "text": {
        "type": "plain_text",
        "text": "{{ title }}",
        "emoji": true
      }
    },
    {
      "type": "section",
      "text": {
        "type": "mrkdwn",
        "text": "{{ body }}"
      },
      "accessory": {
        "type": "button",
        "text": {
          "type": "plain_text",
          "text": "View Details"
        },
        "url": "{{ action_url }}",
        "action_id": "view_details"
      }
    },
    {
      "type": "context",
      "elements": [
        {
          "type": "mrkdwn",
          "text": "*Severity:* {{ severity }} | *Service:* {{ service }} | *Time:* {{ timestamp }}"
        }
      ]
    }
    {% if footer %}
    ,{
      "type": "divider"
    },
    {
      "type": "context",
      "elements": [
        {
          "type": "mrkdwn",
          "text": "{{ footer }}"
        }
      ]
    }
    {% endif %}
  ]
}`

const WELCOME_EMAIL_CONTENT = `<!DOCTYPE html>
<html>
<head><meta charset="utf-8" /><title>Welcome to {{ product_name }}!</title></head>
<body style="font-family: sans-serif; background: #f3f4f6; padding: 24px;">
  <div style="max-width: 600px; margin: 0 auto; background: #fff; border-radius: 8px; padding: 40px;">
    <h1 style="color: #111827; margin-top: 0;">Welcome aboard, {{ first_name }}!</h1>
    <p style="color: #374151; line-height: 1.6;">
      We're thrilled to have you join <strong>{{ product_name }}</strong>. Your account is ready —
      here's everything you need to get started.
    </p>
    <div style="background: #f0fdf4; border: 1px solid #bbf7d0; border-radius: 6px; padding: 20px; margin: 24px 0;">
      <h2 style="color: #15803d; margin-top: 0; font-size: 16px;">Your account details</h2>
      <p style="margin: 4px 0; color: #374151;"><strong>Email:</strong> {{ email }}</p>
      <p style="margin: 4px 0; color: #374151;"><strong>Plan:</strong> {{ plan }}</p>
      {% if trial_days %}
      <p style="margin: 4px 0; color: #374151;"><strong>Trial ends:</strong> {{ trial_end_date }}</p>
      {% endif %}
    </div>
    <a href="{{ onboarding_url }}"
       style="display: inline-block; background: #2563eb; color: #fff; padding: 12px 24px;
              border-radius: 6px; text-decoration: none; font-weight: 600; margin-top: 8px;">
      Start Onboarding
    </a>
    <p style="color: #9ca3af; font-size: 13px; margin-top: 32px;">
      Need help? Reply to this email or visit <a href="{{ support_url }}" style="color: #2563eb;">{{ support_url }}</a>
    </p>
  </div>
</body>
</html>`

const TEMPLATES = [
  {
    id: 'tpl-a1b2c3d4-e5f6-7890-abcd-ef1234567890',
    name: 'alert-body',
    namespace: 'notifications',
    tenant: 'acme-corp',
    content: ALERT_BODY_CONTENT,
    description: 'HTML alert email body with severity-based styling and MiniJinja conditionals',
    created_at: '2026-02-10T08:00:00Z',
    updated_at: '2026-02-18T14:22:00Z',
    labels: { team: 'platform', env: 'prod', type: 'html' },
  },
  {
    id: 'tpl-b2c3d4e5-f6a7-8901-bcde-f12345678901',
    name: 'order-confirmation',
    namespace: 'notifications',
    tenant: 'acme-corp',
    content: ORDER_CONFIRMATION_CONTENT,
    description: 'Order confirmation email with itemized table and MiniJinja for-loops',
    created_at: '2026-02-12T10:30:00Z',
    updated_at: '2026-02-17T09:15:00Z',
    labels: { team: 'commerce', env: 'prod', type: 'html' },
  },
  {
    id: 'tpl-c3d4e5f6-a7b8-9012-cdef-012345678902',
    name: 'slack-notification',
    namespace: 'notifications',
    tenant: 'acme-corp',
    content: SLACK_NOTIFICATION_CONTENT,
    description: 'Slack Block Kit message template with optional footer section',
    created_at: '2026-02-13T11:00:00Z',
    updated_at: '2026-02-16T16:45:00Z',
    labels: { team: 'platform', env: 'prod', type: 'json' },
  },
  {
    id: 'tpl-d4e5f6a7-b8c9-0123-defa-123456789013',
    name: 'welcome-email',
    namespace: 'notifications',
    tenant: 'acme-corp',
    content: WELCOME_EMAIL_CONTENT,
    description: 'Welcome onboarding email with account details and trial information',
    created_at: '2026-02-15T10:30:00Z',
    updated_at: '2026-02-15T10:30:00Z',
    labels: { team: 'growth', env: 'prod', type: 'html' },
  },
]

// ---------------------------------------------------------------------------
// Mock data — Template Profiles
// ---------------------------------------------------------------------------

const PROFILES = [
  {
    id: 'prof-e5f6a7b8-c9d0-1234-efab-234567890124',
    name: 'email-alert',
    namespace: 'notifications',
    tenant: 'acme-corp',
    fields: {
      subject: 'Alert: {{ severity | upper }} - {{ service }} is {{ status }}',
      body: { $ref: 'alert-body' },
      footer: 'This alert was generated by Acteon &bull; <a href="{{ silence_url }}">Silence for 1h</a>',
    },
    description: 'Email profile for operational alerts with severity-aware subject and HTML body',
    created_at: '2026-02-15T10:30:00Z',
    updated_at: '2026-02-18T14:22:00Z',
    labels: { team: 'platform' },
  },
  {
    id: 'prof-f6a7b8c9-d0e1-2345-fabc-345678901235',
    name: 'order-receipt',
    namespace: 'notifications',
    tenant: 'acme-corp',
    fields: {
      subject: 'Your order #{{ order_id }} is confirmed — estimated delivery {{ delivery_date }}',
      html_body: { $ref: 'order-confirmation' },
      reply_to: 'orders@acme.com',
    },
    description: 'Order receipt email profile combining inline subject with a rendered HTML body',
    created_at: '2026-02-16T09:00:00Z',
    updated_at: '2026-02-17T11:30:00Z',
    labels: { team: 'commerce' },
  },
  {
    id: 'prof-a7b8c9d0-e1f2-3456-abcd-456789012346',
    name: 'slack-ops',
    namespace: 'notifications',
    tenant: 'acme-corp',
    fields: {
      channel: '#ops-alerts',
      message: { $ref: 'slack-notification' },
    },
    description: 'Slack profile for ops channel — routes rendered Block Kit blocks to #ops-alerts',
    created_at: '2026-02-17T14:00:00Z',
    updated_at: '2026-02-18T08:45:00Z',
    labels: { team: 'platform' },
  },
]

// ---------------------------------------------------------------------------
// Route mocking helper
// ---------------------------------------------------------------------------

function mockTemplatesApi(
  page: import('@playwright/test').Page,
  opts: {
    templates?: typeof TEMPLATES
    profiles?: typeof PROFILES
  } = {},
) {
  const templates = opts.templates ?? TEMPLATES
  const profiles = opts.profiles ?? PROFILES

  return page.route('**/v1/**', async (route) => {
    const url = route.request().url()

    // Individual template by ID
    const templateIdMatch = url.match(/\/v1\/templates\/([^/?]+)$/)
    if (templateIdMatch && !url.includes('/profiles')) {
      const id = templateIdMatch[1]
      const tpl = templates.find((t) => t.id === id)
      await route.fulfill({
        status: tpl ? 200 : 404,
        contentType: 'application/json',
        body: JSON.stringify(tpl ?? { error: 'not found' }),
      })
      return
    }

    // Individual profile by ID
    const profileIdMatch = url.match(/\/v1\/templates\/profiles\/([^/?]+)$/)
    if (profileIdMatch) {
      const id = profileIdMatch[1]
      const prof = profiles.find((p) => p.id === id)
      await route.fulfill({
        status: prof ? 200 : 404,
        contentType: 'application/json',
        body: JSON.stringify(prof ?? { error: 'not found' }),
      })
      return
    }

    // Profiles list
    if (url.includes('/v1/templates/profiles')) {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ profiles, count: profiles.length }),
      })
      return
    }

    // Templates list
    if (url.match(/\/v1\/templates(\?|$)/)) {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ templates, count: templates.length }),
      })
      return
    }

    // Default: empty list for any other API call
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

test.describe('Template Screenshots', () => {
  test('templates-list — Templates tab with populated data', async ({ page }) => {
    await mockTemplatesApi(page)
    await page.goto(`${BASE_URL}/templates`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'templates-list.png'),
      fullPage: true,
    })
  })

  test('templates-create-modal — Create Template modal filled with realistic data', async ({
    page,
  }) => {
    await mockTemplatesApi(page)
    await page.goto(`${BASE_URL}/templates`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Open the Create Template modal
    await page.getByRole('button', { name: 'Create Template' }).click()
    await page.waitForTimeout(300)

    // Fill in the form fields
    await page.getByLabel('Name *').fill('incident-summary')
    await page.getByLabel('Description').fill('Plain-text incident summary for PagerDuty and email fallback')
    await page.getByLabel('Namespace *').fill('notifications')
    await page.getByLabel('Tenant *').fill('acme-corp')

    // Fill the content textarea
    await page.locator('#template-content').fill(
      `Incident Report — {{ severity | upper }}\n\nService: {{ service }}\nEnvironment: {{ environment }}\nStarted: {{ started_at }}\n\nSummary:\n{{ summary }}\n\n{% if runbook_url %}\nRunbook: {{ runbook_url }}\n{% endif %}\n\nIncident ID: {{ incident_id }}\nReported by: Acteon Gateway`,
    )

    // Fill labels
    await page.locator('#template-labels').fill('team=platform\nenv=prod\ntype=plaintext')

    await page.waitForTimeout(300)

    await page.screenshot({
      path: path.join(ASSETS, 'templates-create-modal.png'),
      fullPage: true,
    })
  })

  test('templates-detail-drawer — Template detail drawer showing content preview', async ({
    page,
  }) => {
    await mockTemplatesApi(page)
    await page.goto(`${BASE_URL}/templates`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Click on the first table row to open the detail drawer for 'alert-body'
    await page.getByRole('row', { name: /alert-body/ }).click()
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'templates-detail-drawer.png'),
      fullPage: true,
    })
  })

  test('templates-delete-confirm — Delete template confirmation modal', async ({ page }) => {
    await mockTemplatesApi(page)
    await page.goto(`${BASE_URL}/templates`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Click the Delete button in the first template row (stopPropagation handled by component)
    await page.getByRole('button', { name: 'Delete template' }).first().click()
    await page.waitForTimeout(300)

    await page.screenshot({
      path: path.join(ASSETS, 'templates-delete-confirm.png'),
      fullPage: true,
    })
  })

  test('profiles-list — Profiles tab with populated data', async ({ page }) => {
    await mockTemplatesApi(page)
    await page.goto(`${BASE_URL}/templates`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Switch to the Profiles tab
    await page.getByRole('tab', { name: 'Profiles' }).click()
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'profiles-list.png'),
      fullPage: true,
    })
  })

  test('profiles-create-modal — Create Profile modal with field builder', async ({ page }) => {
    await mockTemplatesApi(page)
    await page.goto(`${BASE_URL}/templates`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Switch to the Profiles tab
    await page.getByRole('tab', { name: 'Profiles' }).click()
    await page.waitForTimeout(300)

    // Open the Create Profile modal
    await page.getByRole('button', { name: 'Create Profile' }).click()
    await page.waitForTimeout(300)

    // Fill in header fields
    await page.getByLabel('Name *').fill('sms-alert')
    await page.getByLabel('Description').fill('SMS alert profile with inline message and severity prefix')
    await page.getByLabel('Namespace *').fill('notifications')
    await page.getByLabel('Tenant *').fill('acme-corp')

    // Fill the first field row — field name + inline value
    await page.getByLabel('Field 1 name').fill('to')
    await page.getByLabel('Field 1 inline value').fill('{{ phone_number }}')

    // Add second field — change to $ref type
    await page.getByRole('button', { name: 'Add Field' }).click()
    await page.waitForTimeout(200)
    await page.getByLabel('Field 2 name').fill('message')
    // Toggle field 2 to $ref
    await page.getByLabel('Toggle field 2 type (current: inline)').click()
    await page.waitForTimeout(100)
    await page.getByLabel('Field 2 template reference').fill('sms-body')

    // Add third field — another inline field
    await page.getByRole('button', { name: 'Add Field' }).click()
    await page.waitForTimeout(200)
    await page.getByLabel('Field 3 name').fill('from')
    await page.getByLabel('Field 3 inline value').fill('+15550001234')

    // Fill labels
    await page.locator('#profile-labels').fill('team=platform\nenv=prod')

    await page.waitForTimeout(300)

    await page.screenshot({
      path: path.join(ASSETS, 'profiles-create-modal.png'),
      fullPage: true,
    })
  })

  test('profiles-detail-drawer — Profile detail drawer showing fields with $ref badges', async ({
    page,
  }) => {
    await mockTemplatesApi(page)
    await page.goto(`${BASE_URL}/templates`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    // Switch to the Profiles tab
    await page.getByRole('tab', { name: 'Profiles' }).click()
    await page.waitForTimeout(500)

    // Click the first profile row to open the detail drawer (email-alert)
    await page.getByRole('row', { name: /email-alert/ }).click()
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'profiles-detail-drawer.png'),
      fullPage: true,
    })
  })

  test('templates-empty-state — Templates tab with no data', async ({ page }) => {
    await mockTemplatesApi(page, { templates: [], profiles: [] })
    await page.goto(`${BASE_URL}/templates`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'templates-empty-state.png'),
      fullPage: true,
    })
  })
})
