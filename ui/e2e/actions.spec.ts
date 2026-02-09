import { test, expect } from '@playwright/test'

test.describe('Actions / Audit Trail', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/audit')
  })

  test('audit page loads with title', async ({ page }) => {
    await expect(page.getByRole('heading', { name: 'Audit Trail' })).toBeVisible()
  })

  test('filter controls are present', async ({ page }) => {
    await expect(page.getByPlaceholder('Search by ID...')).toBeVisible()
    await expect(page.getByPlaceholder('Namespace')).toBeVisible()
    await expect(page.getByPlaceholder('Tenant')).toBeVisible()
    // Outcome select
    await expect(page.locator('select').first()).toBeVisible()
  })

  test('table renders with correct columns or shows empty state', async ({ page }) => {
    const table = page.locator('table')
    const emptyState = page.getByText('No audit records')

    await expect(table.or(emptyState)).toBeVisible()

    if (await table.isVisible()) {
      const thead = table.locator('thead')
      await expect(thead.getByText('Action ID')).toBeVisible()
      await expect(thead.getByText('Namespace')).toBeVisible()
      await expect(thead.getByText('Tenant')).toBeVisible()
      await expect(thead.getByText('Type')).toBeVisible()
      await expect(thead.getByText('Verdict')).toBeVisible()
      await expect(thead.getByText('Outcome')).toBeVisible()
      await expect(thead.getByText('Duration')).toBeVisible()
    }
  })

  test('empty state shows correct message', async ({ page }) => {
    const emptyState = page.getByText('No audit records')
    if (await emptyState.isVisible()) {
      await expect(page.getByText('Actions are recorded when audit is enabled')).toBeVisible()
    }
  })

  test('filter by outcome works', async ({ page }) => {
    const outcomeSelect = page.locator('select').first()
    await outcomeSelect.selectOption('Executed')
    await expect(page).toHaveURL(/outcome=Executed/)
  })

  test('namespace filter updates URL', async ({ page }) => {
    const nsInput = page.getByPlaceholder('Namespace')
    await nsInput.fill('prod')
    await expect(page).toHaveURL(/namespace=prod/)
  })

  test('click on row opens detail drawer when data exists', async ({ page }) => {
    const tableRows = page.locator('table tbody tr')
    if ((await tableRows.count()) > 0) {
      await tableRows.first().click()
      // Drawer should open with tabs
      await expect(page.getByRole('dialog')).toBeVisible()
      await expect(page.getByText('Overview')).toBeVisible()
      await expect(page.getByText('Payload')).toBeVisible()
    }
  })

  test('detail drawer has replay button when open', async ({ page }) => {
    const tableRows = page.locator('table tbody tr')
    if ((await tableRows.count()) > 0) {
      await tableRows.first().click()
      await expect(page.getByRole('button', { name: /Replay/i })).toBeVisible()
    }
  })
})
