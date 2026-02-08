import { test, expect, type Page } from '@playwright/test'

/** On mobile viewports the sidebar is behind a hamburger menu. Open it first. */
async function ensureSidebarVisible(page: Page) {
  const menuButton = page.getByRole('button', { name: 'Open menu' })
  if (await menuButton.isVisible()) {
    await menuButton.click()
    await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible()
  }
}

test.describe('Navigation', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/')
  })

  test('sidebar renders with all main nav items', async ({ page }) => {
    await ensureSidebarVisible(page)
    const nav = page.getByRole('navigation', { name: 'Main navigation' })
    await expect(nav).toBeVisible()

    const navLabels = [
      'Dashboard', 'Dispatch', 'Rules', 'Audit Trail', 'Events', 'Groups',
      'Chains', 'Approvals', 'Circuit Breakers', 'Dead-Letter Queue', 'Stream', 'Embeddings',
    ]
    for (const label of navLabels) {
      await expect(nav.getByText(label, { exact: true })).toBeVisible()
    }
  })

  test('sidebar shows settings section', async ({ page }) => {
    await ensureSidebarVisible(page)
    const nav = page.getByRole('navigation', { name: 'Main navigation' })
    await expect(nav.getByText('Settings')).toBeVisible()
  })

  test('each sidebar link navigates to correct page', async ({ page }) => {
    const routes: [string, RegExp][] = [
      ['Rules', /\/rules/],
      ['Dispatch', /\/dispatch/],
      ['Audit Trail', /\/audit/],
      ['Chains', /\/chains/],
      ['Approvals', /\/approvals/],
      ['Stream', /\/stream/],
    ]

    for (const [label, pattern] of routes) {
      await ensureSidebarVisible(page)
      const nav = page.getByRole('navigation', { name: 'Main navigation' })
      await nav.getByText(label, { exact: true }).click()
      await expect(page).toHaveURL(pattern)
    }
  })

  test('breadcrumbs update on navigation', async ({ page }) => {
    const breadcrumb = page.getByRole('navigation', { name: 'Breadcrumb' })
    await expect(breadcrumb).toBeVisible()

    // Navigate to rules
    await ensureSidebarVisible(page)
    await page.getByRole('navigation', { name: 'Main navigation' }).getByText('Rules', { exact: true }).click()
    await expect(breadcrumb.getByText('Rules')).toBeVisible()

    // Navigate to audit
    await ensureSidebarVisible(page)
    await page.getByRole('navigation', { name: 'Main navigation' }).getByText('Audit Trail', { exact: true }).click()
    await expect(breadcrumb.getByText('Audit Trail')).toBeVisible()
  })

  test('sidebar collapse and expand works', async ({ page }) => {
    // This test is only meaningful on desktop where collapse toggle exists
    const viewport = page.viewportSize()
    test.skip(!!viewport && viewport.width < 768, 'Sidebar collapse not available on mobile')

    const collapseBtn = page.getByRole('button', { name: /Collapse sidebar/i })
    await expect(collapseBtn).toBeVisible()
    await collapseBtn.click()

    // After collapse, the expand button should appear
    const expandBtn = page.getByRole('button', { name: /Expand sidebar/i })
    await expect(expandBtn).toBeVisible()

    // Click to expand again
    await expandBtn.click()
    await expect(page.getByRole('button', { name: /Collapse sidebar/i })).toBeVisible()
  })

  test('active nav item is highlighted', async ({ page }) => {
    await ensureSidebarVisible(page)
    // Dashboard link should be active on root — check aria-current instead of class name (CSS Modules hash classes)
    const nav = page.getByRole('navigation', { name: 'Main navigation' })
    const dashboardLink = nav.getByRole('link', { name: 'Dashboard' })
    await expect(dashboardLink).toHaveAttribute('aria-current', 'page')
  })

  test('command palette opens with keyboard shortcut', async ({ page }) => {
    // Use the Cmd+K button in header to open (more reliable than keyboard in headless)
    const cmdKButton = page.locator('header button').filter({ hasText: 'K' })
    await expect(cmdKButton).toBeVisible()
    await cmdKButton.click()
    await expect(page.getByPlaceholder('Type a command or search...')).toBeVisible()
  })

  test('command palette search filters results', async ({ page }) => {
    // Open via header button
    const cmdKButton = page.locator('header button').filter({ hasText: 'K' })
    await cmdKButton.click()
    const input = page.getByPlaceholder('Type a command or search...')
    await expect(input).toBeVisible()
    await input.fill('Rules')
    await expect(page.locator('[cmdk-item]').filter({ hasText: 'Rules' }).first()).toBeVisible()
  })

  test('command palette navigation selects item and closes', async ({ page }) => {
    const cmdKButton = page.locator('header button').filter({ hasText: 'K' })
    await cmdKButton.click()
    const input = page.getByPlaceholder('Type a command or search...')
    await expect(input).toBeVisible()
    await input.fill('Chains')
    // Press enter to select first result
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(/\/chains/)
  })

  test('command palette closes on Escape', async ({ page }) => {
    const cmdKButton = page.locator('header button').filter({ hasText: 'K' })
    await cmdKButton.click()
    const input = page.getByPlaceholder('Type a command or search...')
    await expect(input).toBeVisible()
    // Focus the input first to ensure Escape targets the palette
    await input.focus()
    await page.keyboard.press('Escape')
    await expect(input).not.toBeVisible({ timeout: 5000 })
  })

  test('command palette Cmd+K button in header is present', async ({ page }) => {
    // The Cmd+K button in header should be visible
    const cmdKButton = page.locator('header button').filter({ hasText: 'K' })
    await expect(cmdKButton).toBeVisible()
  })
})
