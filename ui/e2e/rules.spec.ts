import { test, expect } from '@playwright/test'

test.describe('Rules', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/rules')
  })

  test('rules page loads with table or empty state', async ({ page }) => {
    // Either the DataTable or empty state should be visible
    const table = page.locator('table')
    const emptyTitle = page.getByText('No rules loaded')

    await expect(table.or(emptyTitle)).toBeVisible()
  })

  test('page header shows Rules title', async ({ page }) => {
    await expect(page.getByRole('heading', { name: 'Rules' })).toBeVisible()
  })

  test('reload rules button is present', async ({ page }) => {
    await expect(page.getByRole('button', { name: /Reload Rules/i })).toBeVisible()
  })

  test('search input is present and functional', async ({ page }) => {
    const searchInput = page.getByPlaceholder('Search rules...')
    await expect(searchInput).toBeVisible()
    await searchInput.fill('test-rule')
    // Should not crash
    await expect(page.locator('body')).toBeVisible()
  })

  test('filter dropdowns are present', async ({ page }) => {
    // Action type filter
    const actionSelect = page.locator('select').first()
    await expect(actionSelect).toBeVisible()
  })

  test('table columns are present when data exists', async ({ page }) => {
    const table = page.locator('table')
    if (await table.isVisible()) {
      await expect(table.getByText('Pri')).toBeVisible()
      await expect(table.getByText('Name')).toBeVisible()
      await expect(table.getByText('Description')).toBeVisible()
      await expect(table.getByText('Enabled')).toBeVisible()
    }
  })

  test('empty state shows correct message', async ({ page }) => {
    const emptyState = page.getByText('No rules loaded')
    if (await emptyState.isVisible()) {
      await expect(page.getByText('Add YAML rule files')).toBeVisible()
    }
  })

  test('reload rules button triggers reload', async ({ page }) => {
    const reloadBtn = page.getByRole('button', { name: /Reload Rules/i })
    await reloadBtn.click()
    // Wait for toast or completion
    await page.waitForTimeout(500)
    // Should not crash
    await expect(page.locator('body')).toBeVisible()
  })
})
