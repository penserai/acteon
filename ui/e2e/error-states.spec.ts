import { test, expect } from '@playwright/test'

test.describe('Error States', () => {
  test('pages handle API errors gracefully - dashboard', async ({ page }) => {
    // Intercept API calls to simulate errors before navigation
    await page.route('**/metrics', (route) =>
      route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"Internal Server Error"}' }),
    )
    await page.route('**/health', (route) =>
      route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"Internal Server Error"}' }),
    )

    await page.goto('/')
    // Wait for the page to handle error state
    await page.waitForTimeout(1000)

    // The page should still load without crashing
    await expect(page.locator('body')).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible()
  })

  test('pages handle API errors gracefully - rules', async ({ page }) => {
    await page.route('**/v1/rules', (route) =>
      route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"Internal Server Error"}' }),
    )

    await page.goto('/rules')
    await page.waitForTimeout(1000)
    await expect(page.locator('body')).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Rules' })).toBeVisible()
  })

  test('pages handle API errors gracefully - audit', async ({ page }) => {
    await page.route('**/v1/audit**', (route) =>
      route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"Internal Server Error"}' }),
    )

    await page.goto('/audit')
    await page.waitForTimeout(1000)
    await expect(page.locator('body')).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Audit Trail' })).toBeVisible()
  })

  test('pages handle API errors gracefully - chains', async ({ page }) => {
    await page.route('**/v1/chains**', (route) =>
      route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"Internal Server Error"}' }),
    )

    await page.goto('/chains')
    await page.waitForTimeout(1000)
    await expect(page.locator('body')).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Chains' })).toBeVisible()
  })

  test('pages handle API errors gracefully - approvals', async ({ page }) => {
    await page.route('**/v1/approvals**', (route) =>
      route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"Internal Server Error"}' }),
    )

    await page.goto('/approvals')
    await page.waitForTimeout(1000)
    await expect(page.locator('body')).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Approvals' })).toBeVisible()
  })

  test('pages handle API errors gracefully - providers', async ({ page }) => {
    await page.route('**/admin/circuit-breakers**', (route) =>
      route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"Internal Server Error"}' }),
    )

    await page.goto('/circuit-breakers')
    await page.waitForTimeout(1000)
    await expect(page.locator('body')).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Providers' })).toBeVisible()
  })

  test('pages handle API errors gracefully - DLQ', async ({ page }) => {
    await page.route('**/v1/dlq/stats**', (route) =>
      route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"Internal Server Error"}' }),
    )

    await page.goto('/dlq')
    await page.waitForTimeout(1000)
    await expect(page.locator('body')).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Dead-Letter Queue' })).toBeVisible()
  })

  test('network errors do not crash the app', async ({ page }) => {
    await page.route('**/metrics', (route) =>
      route.fulfill({ status: 503, contentType: 'text/plain', body: 'Service Unavailable' }),
    )

    await page.goto('/')
    await page.waitForTimeout(1000)
    await expect(page.locator('body')).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible()
  })
})
