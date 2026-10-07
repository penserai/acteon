import { test, expect } from '@playwright/test'
import { readFileSync } from 'node:fs'

const fixture = JSON.parse(readFileSync(new URL('../../clients/contract-fixtures/agent-services.json', import.meta.url), 'utf8'))

test('governed task receipts keep retries and observation bound to the original job', async ({ page }, testInfo) => {
  await page.route('https://fonts.googleapis.com/**', route => route.fulfill({ contentType: 'text/css', body: '' }))
  await page.route('https://fonts.gstatic.com/**', route => route.abort())
  await page.addInitScript(() => localStorage.setItem('acteon-token', 'browser-key'))
  await page.route('**/v1/stream', route => route.fulfill({ contentType: 'text/event-stream', body: ': ready\n\n' }))
  await page.route('**/v1/bus/agents/prod/acme/notifier', route => route.fulfill({ json: {
    namespace: 'prod', tenant: 'acme', agent_id: 'notifier', display_name: 'Incident notifier',
    status: 'online', admin_state: 'active', capabilities: [], inbox_topic: 'notifier',
    heartbeat_ttl_ms: 30000, created_at: '2026-10-07T00:00:00Z', updated_at: '2026-10-07T00:00:00Z',
  } }))
  const messages: { message: { messageId: string; parts: { text: string }[] } }[] = []
  await page.route('**/a2a/prod/acme/agents/notifier/v1/message:send', async route => {
    expect(route.request().headers().authorization).toBe('Bearer browser-key')
    expect(route.request().headers()['x-acteon-agent-source-context']).toBeUndefined()
    messages.push(route.request().postDataJSON())
    if (messages.length === 1) {
      await route.fulfill({ status: 503, json: { error: 'agent_services_unavailable' } })
      return
    }
    const index = messages.length === 2 ? 0 : 1
    const job = fixture.jobs[index]
    await route.fulfill({ json: job.task, headers: { 'a2a-version': '1.0', 'x-acteon-agent-source-context': job.source_context } })
  })
  const observed: string[] = []
  await page.route('**/a2a/prod/acme/agents/notifier/v1/tasks/*', async route => {
    const id = new URL(route.request().url()).pathname.split('/').pop()!
    const job = fixture.jobs.find((job: { task: { id: string } }) => job.task.id === id)
    expect(route.request().headers().authorization).toBe('Bearer browser-key')
    expect(route.request().headers()['x-acteon-agent-source-context']).toBe(job.source_context)
    observed.push(id)
    await route.fulfill({ json: { ...job.task, status: { ...job.task.status, state: 'completed' } }, headers: { 'a2a-version': '1.0' } })
  })
  const stopRequests: { id: string; source: string; body: string | null }[] = []
  await page.route('**/a2a/prod/acme/agents/notifier/v1/tasks/*/stop', async route => {
    const id = new URL(route.request().url()).pathname.split('/').at(-2)!
    const job = fixture.jobs.find((job: { task: { id: string } }) => job.task.id === id)
    expect(route.request().method()).toBe('POST')
    expect(route.request().headers().authorization).toBe('Bearer browser-key')
    expect(route.request().headers()['x-acteon-agent-source-context']).toBe(job.source_context)
    stopRequests.push({ id, source: route.request().headers()['x-acteon-agent-source-context'], body: route.request().postData() })
    if (stopRequests.length === 1) {
      await route.fulfill({ status: 503, json: { error: 'agent_services_unavailable' } })
      return
    }
    if (stopRequests.length === 2) {
      await route.fulfill({ json: { ...job.stop_response, future_starts_blocked: false }, headers: { 'a2a-version': '1.0' } })
      return
    }
    await route.fulfill({ json: job.stop_response, headers: { 'a2a-version': '1.0' } })
  })
  await page.goto('/agents/prod/acme/notifier', { waitUntil: 'domcontentloaded' })
  await page.getByRole('tab', { name: 'Governed tasks', exact: true }).click()
  await page.getByLabel('Message', { exact: true }).fill('Notify incident owner')
  await page.getByRole('button', { name: 'Send request', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('503')
  await expect(page.getByLabel('Message', { exact: true })).toBeDisabled()
  await page.getByRole('button', { name: 'Retry same request', exact: true }).click()
  await expect(page.locator('article').filter({ hasText: 'job-1' })).toBeVisible()
  expect(messages[1]).toEqual(messages[0])
  await page.getByRole('button', { name: 'New request', exact: true }).click()
  await page.getByLabel('Message', { exact: true }).fill('Notify second owner')
  await page.getByRole('button', { name: 'Send request', exact: true }).click()
  await expect(page.locator('article').filter({ hasText: 'job-2' })).toBeVisible()
  expect(messages[2].message.messageId).not.toBe(messages[0].message.messageId)
  const original = page.locator('article').filter({ hasText: 'job-1' })
  await original.getByRole('button', { name: 'Stop future starts', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('503')
  await expect(original).not.toContainText('Future starts stopped.')
  await original.getByRole('button', { name: 'Stop future starts', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Stop acknowledgement unavailable')
  await expect(original).not.toContainText('Future starts stopped.')
  await original.getByRole('button', { name: 'Stop future starts', exact: true }).click()
  await expect(original).toContainText('Future starts stopped.')
  await expect(original).toContainText('submitted')
  await expect(original).not.toContainText('cancelled')
  await expect(original.getByRole('button', { name: 'Stop future starts', exact: true })).toBeDisabled()
  expect(stopRequests).toHaveLength(3)
  expect(stopRequests[1]).toEqual(stopRequests[0])
  expect(stopRequests[2]).toEqual(stopRequests[0])
  await expect(page.locator('article').filter({ hasText: 'job-2' })).not.toContainText('Future starts stopped.')
  for (const id of ['job-2', 'job-1']) {
    const job = page.locator('article').filter({ hasText: id })
    await job.getByRole('button', { name: 'Refresh task', exact: true }).click()
    await expect(job).toContainText('completed')
  }
  expect(observed).toEqual(['job-2', 'job-1'])
  await expect(original).toContainText('Future starts stopped.')
  await expect(original).toContainText('completed')
  for (const job of fixture.jobs) await expect(page.locator('body')).not.toContainText(job.source_context)
  expect(messages).toHaveLength(3)
  const screenshot = `/tmp/acteon-agent-service-${testInfo.project.name}.png`
  await page.screenshot({ path: screenshot, fullPage: true })
  await testInfo.attach('Governed agent tasks', { path: screenshot, contentType: 'image/png' })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
})
