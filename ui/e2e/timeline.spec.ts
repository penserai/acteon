import { test, expect, type Page } from '@playwright/test'

const event = {
  scope: { namespace: 'deployments', tenant: 'acme' },
  source: 'controller', eventId: 'event-1', revision: 2,
  expiresAt: '2026-11-01T12:00:00Z', firstIngestedAt: '2026-10-24T12:00:00Z', updatedAt: '2026-10-24T12:01:00Z',
  event: { eventGroup: 'cluster-42', occurredAt: '2026-10-24T12:00:00Z', eventType: 'deployment.ready', message: 'Cluster ready', attributes: { customId: 'uuid-exact' } },
}
async function openTimeline(page: Page) {
  await page.route('**/v1/stream', route => route.abort())
  await page.goto('/timeline')
  await page.getByLabel('Namespace', { exact: true }).fill('deployments')
  await page.getByLabel('Tenant', { exact: true }).fill('acme')
}

test('timeline sends exact attribute search and shows event details', async ({ page }, testInfo) => {
  await page.route('**/v1/timeline/query', async route => {
    const body = route.request().postDataJSON()
    expect(body.scope).toEqual({ namespace: 'deployments', tenant: 'acme' })
    expect(body.query.equals).toEqual([{ path: '/customId', value: 'uuid-exact' }])
    await route.fulfill({ json: { events: [event], nextCursor: null } })
  })
  await openTimeline(page)
  await page.getByText('Additional filters', { exact: true }).click()
  await page.getByLabel('Attribute path', { exact: true }).fill('/customId')
  await page.getByLabel('Exact string value').fill('uuid-exact')
  await page.getByRole('button', { name: 'Search events' }).click()
  await expect(page.getByText('Cluster ready', { exact: true })).toBeVisible()
  await page.getByText('Additional filters', { exact: true }).click()
  await expect(page.getByRole('button', { name: 'Next page' })).toBeDisabled()
  await expect(page.getByRole('button', { name: 'Previous page' })).toBeDisabled()
  await page.evaluate(() => window.scrollTo(0, 0))
  await page.screenshot({ path: testInfo.outputPath('timeline-results.png'), fullPage: true, animations: 'disabled' })
  await page.evaluate(() => document.documentElement.classList.add('dark'))
  await page.screenshot({ path: testInfo.outputPath('timeline-dark.png'), fullPage: true, animations: 'disabled' })
  await page.evaluate(() => document.documentElement.classList.remove('dark'))
  const result = page.getByRole('button', { name: /deployment.ready/ })
  await result.click()
  const details = page.getByRole('region', { name: 'Event details' })
  await expect(details).toContainText('uuid-exact')
  await expect(details).toContainText('event-1')
  await expect(details).toBeFocused()
  await expect(details.getByRole('button', { name: 'Copy JSON' })).toBeVisible()
  await page.getByRole('button', { name: 'Close event details' }).click()
  await expect(result).toBeFocused()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
})

test('CEL pagination uses the submitted filter, supports previous pages and preserves results on errors', async ({ page }) => {
  const filter = 'attributes.region in ["us-west-2", "eu-west-1"] && !has(attributes.error)'
  let requests = 0
  await page.route('**/v1/timeline/query', async route => {
    const body = route.request().postDataJSON()
    requests++
    if (requests <= 2) {
      expect(body.query.filter).toBe(filter)
      expect(body.query.cursor).toBe(requests === 1 ? undefined : 'page-two')
      await route.fulfill({ json: { events: [{ ...event, eventId: `event-${requests}` }], nextCursor: requests === 1 ? 'page-two' : null } })
    } else {
      expect(body.query.filter).toBe('attributes.attempt > 3')
      await route.fulfill({ status: 400, json: { error: 'CEL filter: range comparisons are supported only for occurredAt' } })
    }
  })
  await openTimeline(page)
  await page.getByLabel('CEL filter').fill(filter)
  await page.getByRole('button', { name: 'Search events' }).click()
  await expect(page.getByRole('button', { name: 'Next page' })).toBeEnabled()
  await page.getByLabel('CEL filter').fill('attributes.attempt > 3')
  await expect(page.getByText(/Filters have changed/)).toBeVisible()
  await page.getByRole('button', { name: 'Next page' }).click()
  await expect(page.getByText('1 event on page 2')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Next page' })).toBeDisabled()
  await page.getByRole('button', { name: 'Previous page' }).click()
  await expect(page.getByText('1 event on page 1')).toBeVisible()
  await page.getByRole('button', { name: 'Next page' }).click()
  await expect(page.getByText('1 event on page 2')).toBeVisible()
  expect(requests).toBe(2)
  await page.getByRole('button', { name: 'Search events' }).click()
  await expect(page.getByRole('alert')).toContainText('range comparisons')
  await expect(page.getByRole('alert')).not.toContainText('{"error"')
  await expect(page.getByRole('alert')).toContainText('Last successful results')
  await expect(page.getByText('1 event on page 2')).toBeVisible()
})

test('typed attribute combinations, text scope and local dates are submitted together', async ({ page }, testInfo) => {
  await page.route('**/v1/timeline/query', async route => {
    const body = route.request().postDataJSON()
    expect(body.query).toMatchObject({
      eventGroup: 'cluster-42', eventType: 'deployment.ready', ascending: false, limit: 100,
      equals: [{ path: '/attempt', value: 2 }, { path: '/ready', value: false }, { path: '/error', value: null }],
      exists: ['/region'], text: { text: 'failed timeout', path: '/description', mode: 'any' },
    })
    const expected = await page.evaluate(() => [new Date('2026-10-24T10:00:00').toISOString(), new Date('2026-10-24T14:00:00').toISOString()])
    expect([body.query.from, body.query.to]).toEqual(expected)
    await route.fulfill({ json: { events: [], nextCursor: null } })
  })
  await openTimeline(page)
  await page.getByLabel('Event group', { exact: true }).fill('cluster-42')
  await page.getByLabel('Event type', { exact: true }).fill('deployment.ready')
  await page.getByLabel('Order', { exact: true }).selectOption('newest')
  await page.getByLabel('Page size').selectOption('100')
  await page.getByText('Additional filters', { exact: true }).click()
  await page.getByLabel('Full-text search').fill('failed timeout')
  await page.getByLabel('Text attribute path').fill('/description')
  await page.getByLabel('Match words').selectOption('any')
  await page.getByLabel('From', { exact: true }).fill('2026-10-24T10:00')
  await page.getByLabel('To', { exact: true }).fill('2026-10-24T14:00')
  for (const [index, path, type, value] of [[1, '/attempt', 'number', '2'], [2, '/ready', 'boolean', 'false'], [3, '/error', 'null', ''], [4, '/region', 'exists', '']] as const) {
    if (index > 1) await page.getByRole('button', { name: 'Add attribute', exact: true }).click()
    const row = page.getByRole('group', { name: `Attribute ${index}`, exact: true })
    await row.getByLabel('Attribute path', { exact: true }).fill(path)
    await row.getByLabel('Value type').selectOption(type)
    if (type === 'number') await row.getByLabel('Exact number value').fill(value)
    if (type === 'boolean') await row.getByLabel('Exact boolean value').selectOption(value)
  }
  await page.evaluate(() => window.scrollTo(0, 0))
  await page.screenshot({ path: testInfo.outputPath('timeline-attributes.png'), fullPage: true, animations: 'disabled' })
  await page.getByRole('button', { name: 'Search events' }).click()
  await expect(page.getByText('No matching events', { exact: true })).toBeVisible()
})

test('invalid dates and numbers are rejected locally; reset keeps the scope', async ({ page }) => {
  let requests = 0
  await page.route('**/v1/timeline/query', async route => { requests++; await route.fulfill({ json: { events: [], nextCursor: null } }) })
  await openTimeline(page)
  await page.getByText('Additional filters', { exact: true }).click()
  await page.getByLabel('From', { exact: true }).fill('2026-10-25T10:00')
  await page.getByLabel('To', { exact: true }).fill('2026-10-24T10:00')
  await page.getByRole('button', { name: 'Search events' }).click()
  await expect(page.getByRole('alert')).toContainText('From must be before')
  await page.getByLabel('To', { exact: true }).fill('')
  await page.getByLabel('Attribute path', { exact: true }).fill('/attempt')
  await page.getByLabel('Value type').selectOption('number')
  await page.getByLabel('Exact number value').fill('not-a-number')
  await page.getByRole('button', { name: 'Search events' }).click()
  await expect(page.getByRole('alert')).toContainText('valid JSON number')
  expect(requests).toBe(0)
  await page.getByLabel('Exact number value').fill('2.0')
  await page.getByRole('button', { name: 'Search events' }).click()
  await expect(page.getByRole('alert')).toContainText('numeric representation')
  expect(requests).toBe(0)
  await page.getByRole('button', { name: 'Reset filters' }).click()
  await expect(page.getByLabel('Namespace', { exact: true })).toHaveValue('deployments')
  await expect(page.getByLabel('Tenant', { exact: true })).toHaveValue('acme')
  await expect(page.getByLabel('From', { exact: true })).toHaveValue('')
  await page.getByRole('button', { name: 'Search events' }).click()
  await expect(page.getByText('No matching events', { exact: true })).toBeVisible()
})

test('scope changes discard an in-flight query and ledger result', async ({ page }) => {
  let releaseQuery!: () => void
  let releaseLedger!: () => void
  const queryGate = new Promise<void>(resolve => { releaseQuery = resolve })
  const ledgerGate = new Promise<void>(resolve => { releaseLedger = resolve })
  await page.route('**/v1/timeline/query', async route => { await queryGate; await route.fulfill({ json: { events: [event], nextCursor: null } }).catch(() => {}) })
  await page.route('**/v1/timeline/ledger/status?**', async route => { await ledgerGate; await route.fulfill({ json: { records: 42, bytes: 100, pending: 0, oldestReceivedAt: null } }).catch(() => {}) })
  await openTimeline(page)
  const queryStarted = page.waitForRequest('**/v1/timeline/query')
  await page.getByRole('button', { name: 'Search events' }).click()
  await queryStarted
  const ledgerStarted = page.waitForRequest('**/v1/timeline/ledger/status?**')
  await page.getByRole('button', { name: 'Ledger status' }).click()
  await ledgerStarted
  const aborted = Promise.all([
    page.waitForEvent('requestfailed', r => r.url().includes('/timeline/query')),
    page.waitForEvent('requestfailed', r => r.url().includes('/timeline/ledger/status')),
  ])
  await page.getByLabel('Tenant', { exact: true }).fill('other')
  releaseQuery(); releaseLedger()
  await aborted
  await expect(page.getByRole('region', { name: 'Timeline results' })).toHaveCount(0)
  await expect(page.getByText(/42 retained CDC/)).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Search events' })).toBeEnabled()
  await expect(page.getByRole('button', { name: 'Ledger status' })).toBeEnabled()
})

test('CEL examples append without executing and keyboard submit works', async ({ page }) => {
  let requests = 0
  await page.route('**/v1/timeline/query', async route => {
    requests++
    expect(route.request().postDataJSON().query.filter).toBe('(eventType == "ready") && (!has(attributes.error))')
    await route.fulfill({ json: { events: [], nextCursor: null } })
  })
  await openTimeline(page)
  await page.getByLabel('CEL filter').fill('eventType == "ready"')
  await page.getByText('CEL examples and syntax', { exact: true }).click()
  await page.getByRole('button', { name: 'Add missing attribute example' }).click()
  expect(requests).toBe(0)
  await expect(page.getByLabel('CEL filter')).toBeFocused()
  await page.getByLabel('CEL filter').press('Control+Enter')
  await expect(page.getByText('No matching events', { exact: true })).toBeVisible()
})
