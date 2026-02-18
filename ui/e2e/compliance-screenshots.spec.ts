/**
 * Playwright script to capture Compliance Status screenshots with realistic mock data.
 *
 * Usage: npx playwright test compliance-screenshots --project chromium
 */
import { test } from '@playwright/test'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)
const ASSETS = path.resolve(__dirname, '../../docs/book/admin-ui/assets')

// ---------------------------------------------------------------------------
// Mock API responses
// ---------------------------------------------------------------------------

const SOC2_STATUS = {
  mode: 'soc2',
  sync_audit_writes: true,
  immutable_audit: false,
  hash_chain: true,
}

const HIPAA_STATUS = {
  mode: 'hipaa',
  sync_audit_writes: true,
  immutable_audit: true,
  hash_chain: true,
}

const VERIFY_VALID = {
  valid: true,
  records_checked: 1523,
  first_broken_at: null,
  first_record_id: '019508a3-6f12-7bc1-a1e0-3f8c7d4e29b5',
  last_record_id: '01950d2e-89ab-7de4-b2f3-5c6a8e1d40c7',
}

const VERIFY_BROKEN = {
  valid: false,
  records_checked: 237,
  first_broken_at: '01950b14-2c5d-7f92-84e1-6b3a9d0e58f2',
  first_record_id: '019508a3-6f12-7bc1-a1e0-3f8c7d4e29b5',
  last_record_id: '01950b14-2c5d-7f92-84e1-6b3a9d0e58f2',
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function mockComplianceApi(
  page: import('@playwright/test').Page,
  status: object,
  verifyResponse?: object,
) {
  return page.route('**/v1/**', async (route) => {
    const url = route.request().url()
    if (url.includes('/v1/compliance/status')) {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(status),
      })
    } else if (url.includes('/v1/compliance/verify-chain') && verifyResponse) {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(verifyResponse),
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

// Use production build for reliable CSS rendering
const BASE_URL = 'http://localhost:4173'

test.use({ viewport: { width: 1440, height: 900 } })

// ---------------------------------------------------------------------------
// Tests (each produces one screenshot)
// ---------------------------------------------------------------------------

test.describe('Compliance Status Screenshots', () => {
  test('SOC2 mode with all features', async ({ page }) => {
    await mockComplianceApi(page, SOC2_STATUS)
    await page.goto(`${BASE_URL}/compliance`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'compliance-soc2.png'),
      fullPage: true,
    })
  })

  test('HIPAA mode with all features', async ({ page }) => {
    await mockComplianceApi(page, HIPAA_STATUS)
    await page.goto(`${BASE_URL}/compliance`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'compliance-hipaa.png'),
      fullPage: true,
    })
  })

  test('hash chain verification — valid', async ({ page }) => {
    await mockComplianceApi(page, SOC2_STATUS, VERIFY_VALID)
    await page.goto(`${BASE_URL}/compliance`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(300)

    // Fill in the verification form
    const nsInput = page.getByLabel('Namespace')
    await nsInput.fill('notifications')

    const tenantInput = page.getByLabel('Tenant')
    await tenantInput.fill('acme-corp')

    // Click Verify and wait for results
    await page.getByRole('button', { name: /Verify/i }).click()
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'compliance-verify-valid.png'),
      fullPage: true,
    })
  })

  test('hash chain verification — broken', async ({ page }) => {
    await mockComplianceApi(page, SOC2_STATUS, VERIFY_BROKEN)
    await page.goto(`${BASE_URL}/compliance`, { waitUntil: 'networkidle' })
    await page.waitForTimeout(300)

    // Fill in the verification form
    const nsInput = page.getByLabel('Namespace')
    await nsInput.fill('notifications')

    const tenantInput = page.getByLabel('Tenant')
    await tenantInput.fill('acme-corp')

    // Click Verify and wait for results
    await page.getByRole('button', { name: /Verify/i }).click()
    await page.waitForTimeout(500)

    await page.screenshot({
      path: path.join(ASSETS, 'compliance-verify-broken.png'),
      fullPage: true,
    })
  })
})
