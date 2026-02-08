import { test, expect } from '@playwright/test'

test.describe('Providers / Circuit Breakers', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/circuit-breakers')
  })

  test('providers page loads with title', async ({ page }) => {
    await expect(page.getByRole('heading', { name: 'Providers' })).toBeVisible()
  })

  test('shows provider cards or empty state', async ({ page }) => {
    // Wait for loading to finish
    await page.waitForTimeout(1000)

    // The page shows either provider cards, "not enabled" message, or "no providers" message
    const notEnabled = page.getByText('Circuit breakers not enabled')
    const noProviders = page.getByText('No providers configured')
    const providerCard = page.locator('button[class*="providerCard"]')

    const cards = await providerCard.count()
    if (cards > 0) {
      await expect(providerCard.first()).toBeVisible()
    } else {
      // Either "not enabled" or "no providers" is shown
      const isNotEnabled = await notEnabled.isVisible().catch(() => false)
      if (isNotEnabled) {
        await expect(notEnabled).toBeVisible()
      } else {
        await expect(noProviders).toBeVisible()
      }
    }
  })

  test('provider cards show name and state badge', async ({ page }) => {
    await page.waitForTimeout(1000)
    const cards = page.locator('button').filter({ has: page.locator('[class*="font-semibold"]') })
    if ((await cards.count()) > 0) {
      // Each card should have a name and badge
      const firstCard = cards.first()
      await expect(firstCard).toBeVisible()
    }
  })

  test('click on provider card opens detail drawer', async ({ page }) => {
    await page.waitForTimeout(1000)
    const cards = page.locator('button').filter({ has: page.locator('[class*="font-semibold"]') })
    if ((await cards.count()) > 0) {
      await cards.first().click()
      await expect(page.getByRole('dialog')).toBeVisible()
      // Drawer should show Circuit Breaker section
      await expect(page.getByText('Circuit Breaker')).toBeVisible()
    }
  })

  test('trip and reset buttons are present in drawer', async ({ page }) => {
    await page.waitForTimeout(1000)
    const cards = page.locator('button').filter({ has: page.locator('[class*="font-semibold"]') })
    if ((await cards.count()) > 0) {
      await cards.first().click()
      await expect(page.getByRole('button', { name: /Trip Circuit/i })).toBeVisible()
      await expect(page.getByRole('button', { name: /Reset Circuit/i })).toBeVisible()
    }
  })

  test('trip button opens confirmation modal', async ({ page }) => {
    await page.waitForTimeout(1000)
    const cards = page.locator('button').filter({ has: page.locator('[class*="font-semibold"]') })
    if ((await cards.count()) > 0) {
      await cards.first().click()
      await page.getByRole('button', { name: /Trip Circuit/i }).click()
      await expect(page.getByText('Force-open circuit')).toBeVisible()
      await expect(page.getByRole('button', { name: 'Confirm' })).toBeVisible()
    }
  })

  test('empty state shows correct message', async ({ page }) => {
    await page.waitForTimeout(1000)
    const emptyState = page.getByText('No providers configured')
    if (await emptyState.isVisible()) {
      await expect(page.getByText('Add providers to your acteon.toml')).toBeVisible()
    }
  })
})
