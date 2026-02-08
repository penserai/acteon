import { test, expect } from '@playwright/test'

test.describe('Approvals', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/approvals')
  })

  test('approvals page loads with title', async ({ page }) => {
    await expect(page.getByRole('heading', { name: 'Approvals' })).toBeVisible()
  })

  test('filter inputs are present', async ({ page }) => {
    await expect(page.getByPlaceholder('Namespace')).toBeVisible()
    await expect(page.getByPlaceholder('Tenant')).toBeVisible()
  })

  test('shows approval cards or empty state', async ({ page }) => {
    // Wait for loading
    await page.waitForTimeout(1000)

    const emptyState = page.getByText('No pending approvals')
    const approvalCards = page.locator('article')

    if ((await approvalCards.count()) > 0) {
      await expect(approvalCards.first()).toBeVisible()
    } else {
      await expect(emptyState).toBeVisible()
    }
  })

  test('approval cards show approve and reject buttons when data exists', async ({ page }) => {
    await page.waitForTimeout(1000)
    const approvalCards = page.locator('article')
    if ((await approvalCards.count()) > 0) {
      await expect(page.getByRole('button', { name: /Approve/i }).first()).toBeVisible()
      await expect(page.getByRole('button', { name: /Reject/i }).first()).toBeVisible()
    }
  })

  test('empty state shows helpful description', async ({ page }) => {
    await page.waitForTimeout(1000)
    const emptyState = page.getByText('No pending approvals')
    if (await emptyState.isVisible()) {
      await expect(page.getByText('Actions requiring approval will appear here')).toBeVisible()
    }
  })

  test('approval cards show action info when data exists', async ({ page }) => {
    await page.waitForTimeout(1000)
    const approvalCards = page.locator('article')
    if ((await approvalCards.count()) > 0) {
      const card = approvalCards.first()
      // Should show action ID
      await expect(card.getByText('Action:')).toBeVisible()
      // Should show rule
      await expect(card.getByText('Rule:')).toBeVisible()
    }
  })
})
