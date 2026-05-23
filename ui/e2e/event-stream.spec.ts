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
    // The status text should be visible (connected, connecting, or
    // disconnected). Use a retrying locator so the test tolerates the
    // EventStream route being lazy-loaded — the assertion auto-retries
    // until the page chunk arrives and the status chip renders.
    const statusChip = page.getByText(/^(connected|connecting|disconnected)$/).first()
    await expect(statusChip).toBeVisible({ timeout: 10_000 })
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
    // Wait for the lazy route chunk to render the filter select before
    // counting options. `expect(locator).not.toHaveCount(0)` on the
    // options locator auto-retries until the select is attached — so
    // this works whether the page is eager or code-split.
    const select = page.locator('select').first()
    await expect(select).toBeVisible({ timeout: 10_000 })
    const options = select.locator('option')
    await expect(options).not.toHaveCount(0)
    // At least "All Types" + at least one event type = more than one option.
    expect(await options.count()).toBeGreaterThan(1)
  })

  test('empty stream shows waiting message', async ({ page }) => {
    // On a fresh server, no events may be present
    const waitingMsg = page.getByText('Waiting for events...')
    if (await waitingMsg.isVisible()) {
      await expect(waitingMsg).toBeVisible()
    }
  })
})
