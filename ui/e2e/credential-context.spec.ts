import { test, expect } from '@playwright/test'

test('dashboard stream sends credentials and reconnects after a credential change', async ({ page }) => {
  await page.addInitScript(() => {
    if (!localStorage.getItem('acteon-token')) localStorage.setItem('acteon-token', 'tenant-a-key')
  })
  await page.route('**/v1/auth/identity', route => route.fulfill({ json: {
    credential_id: 'viewer', auth_method: 'api_key', role: 'viewer', principal: null,
    scope: { namespace: 'payments', tenant: route.request().headers().authorization?.endsWith('tenant-b-key') ? 'b' : 'a' },
  } }))
  const credentials: string[] = []
  await page.route('**/v1/stream', async route => {
    credentials.push(route.request().headers().authorization)
    expect(new URL(route.request().url()).search).toBe('')
    await route.fulfill({ contentType: 'text/event-stream', body: ': authenticated\n\n' })
  })
  await page.goto('/timeline')
  await expect.poll(() => credentials).toContain('Bearer tenant-a-key')
  await page.evaluate(() => {
    localStorage.setItem('acteon-token', 'tenant-b-key')
    window.dispatchEvent(new Event('focus'))
  })
  await expect.poll(() => credentials).toContain('Bearer tenant-b-key')
  await expect(page.getByLabel('Namespace', { exact: true })).toHaveCount(0)
  await expect(page.getByLabel('Tenant', { exact: true })).toHaveCount(0)
})
