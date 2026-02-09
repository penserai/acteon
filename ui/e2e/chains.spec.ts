import { test, expect } from '@playwright/test'

test.describe('Chains', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/chains')
  })

  test('chains page loads with title', async ({ page }) => {
    await expect(page.getByRole('heading', { name: 'Chains' })).toBeVisible()
  })

  test('filter inputs are present', async ({ page }) => {
    await expect(page.getByPlaceholder('Namespace')).toBeVisible()
    await expect(page.getByPlaceholder('Tenant')).toBeVisible()
  })

  test('status filter dropdown is present', async ({ page }) => {
    const statusSelect = page.locator('select')
    await expect(statusSelect.first()).toBeVisible()
  })

  test('chains list shows table or empty state', async ({ page }) => {
    const table = page.locator('table')
    const emptyState = page.getByText('No chain executions')

    await expect(table.or(emptyState)).toBeVisible()
  })

  test('table has correct columns when data exists', async ({ page }) => {
    const table = page.locator('table')
    if (await table.isVisible()) {
      await expect(table.getByText('Chain ID')).toBeVisible()
      await expect(table.getByText('Name')).toBeVisible()
      await expect(table.getByText('Status')).toBeVisible()
      await expect(table.getByText('Progress')).toBeVisible()
      await expect(table.getByText('Started')).toBeVisible()
    }
  })

  test('empty state shows helpful message', async ({ page }) => {
    const emptyState = page.getByText('No chain executions')
    if (await emptyState.isVisible()) {
      await expect(page.getByText('Chain executions are created when a rule triggers a Chain action')).toBeVisible()
    }
  })

  test('status filter works', async ({ page }) => {
    const statusSelect = page.locator('select').first()
    await statusSelect.selectOption('completed')
    await expect(page).toHaveURL(/status=completed/)
  })

  test('click on chain row navigates to detail page when data exists', async ({ page }) => {
    const tableRows = page.locator('table tbody tr')
    if ((await tableRows.count()) > 0) {
      await tableRows.first().click()
      await expect(page).toHaveURL(/\/chains\//)
    }
  })
})
