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

test('governance preserves scoped authority and reviewed recovery', async ({ page }) => {
  const { readFileSync } = await import('node:fs')
  const fixture = JSON.parse(readFileSync(new URL('../../clients/contract-fixtures/governance-management.json', import.meta.url), 'utf8'))
  const registryFixture = JSON.parse(readFileSync(new URL('../../clients/contract-fixtures/governance-registry.json', import.meta.url), 'utf8'))
  const scope = structuredClone(fixture.scope)
  let registryView = structuredClone(registryFixture.view)
  const resource = scope.routes[0].effect.resources[0]
  // Exercise a realistic long endpoint identity on both desktop and mobile.
  resource.id = 'endpoint/' + '0123456789abcdef'.repeat(8)
  scope.closed_resources = []
  scope.routes[0].closed = false
  await page.addInitScript(() => localStorage.setItem('acteon-token', 'operator-key'))
  const changes: Record<string, unknown>[] = []
  const publications: Record<string, unknown>[] = []
  const registryChanges: Record<string, unknown>[] = []
  let registryReads = 0
  let redirectedReads = 0
  await page.route('**/redirected-registry**', async route => { redirectedReads++; await route.fulfill({ status: 500 }) })
  await page.route('**/v1/governance**', async route => {
    const request = route.request()
    expect(request.headers()['authorization']).toBe('Bearer operator-key')
    const url = new URL(request.url())
    if (url.pathname.startsWith('/v1/governance/registry')) {
      if (request.method() === 'GET') {
        registryReads++
        expect(url.pathname).toBe('/v1/governance/registry/maya')
        expect(Object.fromEntries(url.searchParams)).toEqual({ namespace: 'prod', tenant: 'acme', projection: 'card' })
        if (registryReads === 1) {
          await route.fulfill({ status: 307, headers: { location: '/redirected-registry' } })
        } else if (registryReads === 2) {
          await route.fulfill({ json: { ...registryView, tenant: 'other' } })
        } else await route.fulfill({ json: registryView })
        return
      }
      const body = request.postDataJSON()
      registryChanges.push(body)
      if (registryChanges.length === 1) {
        await route.fulfill({ status: 307, headers: { location: '/redirected-registry' } }); return
      }
      if (registryChanges.length === 2) {
        await route.fulfill({ status: 503, json: { error: 'registry_unavailable' } }); return
      }
      if (registryChanges.length === 3) {
        await route.fulfill({ json: { ...registryFixture.receipt, change_id: body.change_id, applied: false } }); return
      }
      registryView = { ...registryView, qualification_retired: true,
        version: body.value === null ? null : (registryView.version ?? 0) + 1, value: body.value }
      await route.fulfill({ json: { ...registryFixture.receipt, change_id: body.change_id,
        expected_registry_revision: body.expected_registry_revision } })
      return
    }
    if (request.method() === 'GET') {
      expect(url.searchParams.get('namespace')).toBe('prod')
      expect(url.searchParams.get('tenant')).toBe('acme')
      await route.fulfill({ json: scope })
      return
    }
    const body = request.postDataJSON()
    expect(body.namespace).toBe('prod')
    expect(body.tenant).toBe('acme')
    expect(body.reason).toBe('Scheduled maintenance')
    if (request.url().endsWith('/permits')) {
      publications.push(body)
      expect(body.expected_revision).toBe(0)
      expect(body.permit).toEqual({
        id: 'new-maya', revision: 1, subject: { id: 'agent/maya', kind: 'agent' },
        routes: [{ provider: 'incident', action_type: 'execute' }], valid_from_ms: 0,
        limits: { max_units: 2, max_concurrent: 1, deadline_ms: new Date('2030-01-01T12:00').getTime() },
      })
      if (publications.length === 1) {
        await route.fulfill({ status: 503, json: { error: 'governance_unavailable' } })
      } else {
        await route.fulfill({ json: { ...fixture.receipt, change_id: body.change_id, reason: body.reason } })
      }
      return
    }
    expect(body.change.resource).toEqual(resource)
    changes.push(body)
    scope.routes[0].closed = body.change.kind === 'close_resource'
    scope.closed_resources = scope.routes[0].closed ? [resource] : []
    await route.fulfill({ json: { ...fixture.receipt, change_id: body.change_id, reason: body.reason } })
  })
  await page.goto('/governance')
  await page.getByLabel('Namespace', { exact: true }).fill('prod')
  await page.getByLabel('Tenant', { exact: true }).fill('acme')
  await page.getByRole('button', { name: 'Inspect scope', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Governed routes' })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Issue a permit' })).toBeVisible()
  const registry = page.getByRole('region', { name: 'Registry metadata' })
  await registry.getByLabel('Registry agent ID').fill('maya')
  await registry.getByRole('button', { name: 'Inspect registry record' }).click()
  await expect(registry.getByRole('alert')).toBeVisible()
  expect(redirectedReads).toBe(0)
  await registry.getByRole('button', { name: 'Inspect registry record' }).click()
  await expect(registry.getByRole('alert')).toContainText('identity or version mismatch')
  await registry.getByRole('button', { name: 'Inspect registry record' }).click()
  await expect(registry.getByText('Registry revision 7')).toBeVisible()
  await registry.getByLabel('Projection JSON').fill(JSON.stringify({ ...registryFixture.view.value, name: 'Maya responder' }, null, 2))
  await registry.getByLabel('Registry change reason').fill('Publish reviewed responder card')
  await registry.getByRole('button', { name: 'Review metadata update' }).click()
  await expect(registry.getByLabel('Projection JSON')).toBeDisabled()
  await registry.getByRole('button', { name: 'Apply reviewed registry change' }).evaluate(button => { button.click(); button.click() })
  await expect(registry.getByRole('alert')).toBeVisible()
  expect(redirectedReads).toBe(0)
  expect(registryChanges).toHaveLength(1)
  await expect(page.getByRole('button', { name: 'Inspect scope', exact: true })).toBeDisabled()
  await registry.getByRole('button', { name: 'Retry same registry change' }).click()
  await expect(registry.getByRole('alert')).toContainText('503')
  expect(registryChanges).toHaveLength(2)
  expect(await page.evaluate(() => localStorage.getItem('acteon-registry-intent:prod:acme'))).not.toBeNull()
  await page.reload()
  await page.getByLabel('Namespace', { exact: true }).fill('prod')
  await page.getByLabel('Tenant', { exact: true }).fill('acme')
  await page.getByRole('button', { name: 'Inspect scope', exact: true }).click()
  await expect(registry.getByRole('status').filter({ hasText: 'Recovered the exact reviewed request' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Inspect scope', exact: true })).toBeDisabled()
  await expect(registry.getByRole('button', { name: 'Discard and inspect again' })).toBeVisible()
  await registry.getByRole('button', { name: 'Retry same registry change' }).click()
  await expect(registry.getByRole('alert')).toContainText('Unmatched or incomplete')
  await registry.getByRole('button', { name: 'Retry same registry change' }).click()
  await expect(registry.getByRole('status').filter({ hasText: 'Registry change applied' })).toBeVisible()
  expect(registryChanges).toHaveLength(4)
  expect(registryChanges[1]).toEqual(registryChanges[0])
  expect(registryChanges[2]).toEqual(registryChanges[0])
  expect(registryChanges[3]).toEqual(registryChanges[0])
  expect(await page.evaluate(() => localStorage.getItem('acteon-registry-intent:prod:acme'))).toBeNull()
  expect(registryChanges[0]).toMatchObject({ namespace: 'prod', tenant: 'acme', agent_id: 'maya', projection: 'card',
    expected_registry_revision: 7, expected_projection_version: 3, reason: 'Publish reviewed responder card',
    value: { name: 'Maya responder' } })
  await registry.getByRole('button', { name: 'Inspect registry record' }).click()
  await expect(registry.getByText('backend version 4')).toBeVisible()
  await registry.getByLabel('Registry change reason').fill('Remove retired responder card')
  await registry.getByRole('button', { name: 'Review metadata removal' }).click()
  await registry.getByRole('button', { name: 'Apply reviewed registry change' }).click()
  await expect(registry.getByRole('status').filter({ hasText: 'Registry change applied' })).toBeVisible()
  expect(registryChanges).toHaveLength(5)
  expect(registryChanges[4]).toMatchObject({ agent_id: 'maya', expected_registry_revision: 7,
    expected_projection_version: 4, value: null, reason: 'Remove retired responder card' })
  expect(registryChanges[4].change_id).not.toBe(registryChanges[0].change_id)
  const widths = await page.evaluate(() => ({ content: document.documentElement.scrollWidth, viewport: document.documentElement.clientWidth }))
  expect(widths.content).toBeLessThanOrEqual(widths.viewport)
  await page.getByRole('button', { name: 'Close', exact: true }).first().click()
  await expect(page.getByRole('button', { name: 'Apply change' })).toBeDisabled()
  expect(changes).toHaveLength(0)
  await page.getByLabel('Reason', { exact: true }).fill('Scheduled maintenance')
  await page.getByRole('button', { name: 'Apply change' }).click()
  await expect(page.getByRole('status').filter({ hasText: 'Control recorded' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Reopen', exact: true })).toBeVisible()
  expect(changes).toHaveLength(1)
  expect((changes[0].change as { kind: string }).kind).toBe('close_resource')
  await page.getByRole('button', { name: 'Reopen', exact: true }).click()
  await page.getByLabel('Reason', { exact: true }).fill('Scheduled maintenance')
  await page.getByRole('button', { name: 'Apply change' }).click()
  await expect(page.getByRole('button', { name: 'Reopen', exact: true })).toHaveCount(0)
  expect(changes).toHaveLength(2)
  expect((changes[1].change as { kind: string }).kind).toBe('reopen_resource')
  expect(changes[0].change_id).not.toBe(changes[1].change_id)
  await expect(page.getByRole('dialog')).toHaveCount(0)
  await page.getByLabel('Permit ID', { exact: true }).fill('new-maya')
  await page.getByLabel('Subject', { exact: true }).selectOption('agent/maya')
  await page.getByLabel('Route', { exact: true }).selectOption({ index: 1 })
  await page.getByLabel('Deadline', { exact: true }).fill('2030-01-01T12:00')
  await page.getByLabel('Maximum calls per root', { exact: true }).fill('2')
  await page.getByLabel('Issuance reason', { exact: true }).fill('Scheduled maintenance')
  await page.getByRole('button', { name: 'Issue permit', exact: true }).click()
  await expect(page.getByRole('alert')).toBeVisible()
  expect(publications).toHaveLength(1)
  await page.getByRole('button', { name: 'Issue permit', exact: true }).click()
  await expect(page.getByLabel('Permit ID', { exact: true })).toHaveValue('')
  expect(publications).toHaveLength(2)
  expect(publications[0].change_id).toBe(publications[1].change_id)
  await page.screenshot({ path: test.info().outputPath('governance.png'), fullPage: true, animations: 'disabled' })
})

test('workforce preserves a reviewed request through manual retry and offboarding', async ({ page }) => {
  const { readFileSync } = await import('node:fs')
  const fixture = JSON.parse(readFileSync(new URL('../../clients/contract-fixtures/workforce-management.json', import.meta.url), 'utf8'))
  const view = structuredClone(fixture.scope)
  view.management.limits.max_units = 2
  view.management.limits.deadline_ms = 4000000000000
  const changes: Record<string, unknown>[] = []
  let failOnce = true
  await page.addInitScript(() => localStorage.setItem('acteon-token', 'workforce-ui-key'))
  await page.route('**/v1/workforce**', async route => {
    expect(route.request().headers()['authorization']).toBe('Bearer workforce-ui-key')
    if (route.request().method() === 'GET') {
      const url = new URL(route.request().url())
      expect(url.searchParams.get('namespace')).toBe('prod')
      expect(url.searchParams.get('tenant')).toBe('acme')
      await route.fulfill({ json: view }); return
    }
    const body = route.request().postDataJSON()
    changes.push(body)
    if (failOnce) { failOnce = false; await route.fulfill({ status: 503, json: { error: 'unavailable' } }); return }
    if (body.change.kind === 'put_team') view.teams[0].value = body.change.team
    if (body.change.kind === 'remove_membership') view.memberships[0].revoked = true
    await route.fulfill({ json: { ...fixture.receipt, change_id: body.change_id, reason: body.reason, generation: 20 } })
  })
  await page.goto('/workforce')
  await page.getByLabel('Namespace', { exact: true }).fill('prod')
  await page.getByLabel('Tenant', { exact: true }).fill('acme')
  await page.getByRole('button', { name: 'Inspect workforce' }).click()
  await expect(page.getByRole('heading', { name: 'Teams', exact: true })).toBeVisible()
  await page.getByLabel('Team', { exact: true }).selectOption(JSON.stringify(fixture.scope.management.teams[0]))
  await page.getByLabel('Team name', { exact: true }).fill('Reliability Operations')
  await page.getByRole('button', { name: 'Review workforce record' }).click()
  await page.getByLabel('Reason', { exact: true }).fill('Rename the incident response team')
  await page.getByRole('button', { name: 'Apply change', exact: true }).click()
  await expect(page.getByRole('alert').filter({ hasText: '503' })).toBeVisible()
  await expect(page.getByLabel('Reason', { exact: true })).toBeDisabled()
  expect(changes).toHaveLength(1)
  await page.getByRole('button', { name: 'Retry same change', exact: true }).click()
  await expect(page.getByRole('status').filter({ hasText: 'Workforce change recorded' })).toBeVisible()
  expect(changes).toHaveLength(2)
  expect(changes[1]).toEqual(changes[0])
  expect(changes[0]).toMatchObject({ namespace: 'prod', tenant: 'acme', change: {
    kind: 'put_team', team: { team: fixture.scope.management.teams[0], revision: 2, name: 'Reliability Operations' } } })
  await page.getByLabel('Mandate', { exact: true }).selectOption('maya-team')
  await page.getByLabel('Permit ID', { exact: true }).fill('bounded-permit')
  await page.getByRole('button', { name: 'Review represented permit' }).click()
  await page.getByLabel('Reason', { exact: true }).fill('Authorize bounded incident work')
  await page.getByRole('button', { name: 'Apply change', exact: true }).click()
  await expect(page.getByRole('dialog')).toHaveCount(0)
  expect(changes[2]).toMatchObject({ change: { kind: 'publish_represented_permit',
    permit: { subject: { id: 'agent/maya', kind: 'agent' }, limits: { max_units: 2, max_concurrent: 1, deadline_ms: 4000000000000 } },
    mandate: { id: 'maya-team', accepted_revision: 1 } } })
  await page.getByRole('button', { name: 'Remove membership', exact: true }).click()
  await page.getByLabel('Reason', { exact: true }).fill('End this team assignment')
  await page.getByRole('button', { name: 'Apply change', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Removed', exact: true })).toBeDisabled()
  expect(changes[3]).toMatchObject({ change: { kind: 'remove_membership', id: 'maya-reliability', expected_revision: 1 } })
  await page.screenshot({ path: test.info().outputPath('workforce.png'), fullPage: true, animations: 'disabled' })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
})
