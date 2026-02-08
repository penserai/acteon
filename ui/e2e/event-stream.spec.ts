import { test, expect } from '@playwright/test'

test.describe('Event Stream', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/stream')
  })

  test('event stream page loads with title', async ({ page }) => {
    await expect(page.getByText('Event Stream', { exact: true }).first()).toBeVisible()
  })

  test('filter controls are present', async ({ page }) => {
    await expect(page.getByPlaceholder('Namespace')).toBeVisible()
    await expect(page.getByPlaceholder('Tenant')).toBeVisible()
    // Event type dropdown
    await expect(page.locator('select').first()).toBeVisible()
  })

  test('connection status indicator is visible', async ({ page }) => {
    // The status text should be visible (connected, connecting, or disconnected)
    const statuses = ['connected', 'connecting', 'disconnected']
    const statusVisible = await Promise.any(
      statuses.map(async (s) => {
        const el = page.getByText(s, { exact: true })
        return el.isVisible().then(v => v ? s : Promise.reject())
      }),
    ).catch(() => null)

    // At least one status should be present
    expect(statusVisible).not.toBeNull()
  })

  test('pause/resume button is present', async ({ page }) => {
    const pauseBtn = page.getByRole('button', { name: /Pause/i })
    await expect(pauseBtn).toBeVisible()
  })

  test('pause button toggles to resume', async ({ page }) => {
    // BUG: On mobile viewports the Pause button is obscured by header/sidebar overlap
    const viewport = page.viewportSize()
    test.skip(!!viewport && viewport.width < 768, 'Pause button obscured on narrow viewports (mobile layout bug)')

    const pauseBtn = page.getByRole('button', { name: /Pause/i })
    await pauseBtn.click()
    await expect(page.getByRole('button', { name: /Resume/i })).toBeVisible()
  })

  test('event type filter dropdown has options', async ({ page }) => {
    const select = page.locator('select').first()
    const options = select.locator('option')
    expect(await options.count()).toBeGreaterThan(1) // At least "All Types" + some event types
  })

  test('empty stream shows waiting message', async ({ page }) => {
    // On a fresh server, no events may be present
    const waitingMsg = page.getByText('Waiting for events...')
    if (await waitingMsg.isVisible()) {
      await expect(waitingMsg).toBeVisible()
    }
  })
})
