import { test, expect } from '@playwright/test'

// Run these tests at a mobile viewport
test.describe('Responsive Design', () => {
  test.use({ viewport: { width: 375, height: 667 } })

  test('dashboard renders at mobile viewport', async ({ page }) => {
    await page.goto('/')
    await expect(page.locator('body')).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible()
  })

  test('sidebar is visible at mobile viewport', async ({ page }) => {
    await page.goto('/')
    // On mobile, sidebar is hidden behind hamburger menu
    const menuButton = page.getByRole('button', { name: 'Open menu' })
    await expect(menuButton).toBeVisible()
    // Open the mobile sidebar
    await menuButton.click()
    const nav = page.getByRole('navigation', { name: 'Main navigation' })
    await expect(nav).toBeVisible()
  })

  test('rules page renders at mobile viewport', async ({ page }) => {
    await page.goto('/rules')
    await expect(page.getByRole('heading', { name: 'Rules' })).toBeVisible()
  })

  test('audit trail renders at mobile viewport', async ({ page }) => {
    await page.goto('/audit')
    await expect(page.getByRole('heading', { name: 'Audit Trail' })).toBeVisible()
  })

  test('chains page renders at mobile viewport', async ({ page }) => {
    await page.goto('/chains')
    await expect(page.getByRole('heading', { name: 'Chains' })).toBeVisible()
  })

  test('dispatch form renders at mobile viewport', async ({ page }) => {
    await page.goto('/dispatch')
    await expect(page.getByRole('heading', { name: 'Dispatch Action' })).toBeVisible()
    // Form fields should be usable
    await expect(page.getByLabel('Namespace *')).toBeVisible()
  })

  test('approvals page renders at mobile viewport', async ({ page }) => {
    await page.goto('/approvals')
    await expect(page.getByRole('heading', { name: 'Approvals' })).toBeVisible()
  })

  test('providers page renders at mobile viewport', async ({ page }) => {
    await page.goto('/circuit-breakers')
    await expect(page.getByRole('heading', { name: 'Providers' })).toBeVisible()
  })

  test('settings page renders at mobile viewport', async ({ page }) => {
    await page.route('**/admin/config', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ server: { host: '127.0.0.1', port: 8080, shutdown_timeout_seconds: 30, external_url: null, max_sse_connections_per_tenant: null }, state: { backend: 'memory', has_url: false, prefix: null, region: null, table_name: null }, executor: { max_retries: null, timeout_seconds: null, max_concurrent: null, dlq_enabled: false }, rules: { directory: null, default_timezone: null }, audit: { enabled: false, backend: 'memory', has_url: false, prefix: '', ttl_seconds: null, cleanup_interval_seconds: 3600, store_payload: true, redact: { enabled: false, field_count: 0, placeholder: '[REDACTED]' } }, auth: { enabled: false, config_path: null, watch: null }, rate_limit: { enabled: false, config_path: null, on_error: 'allow' }, llm_guardrail: { enabled: false, endpoint: '', model: '', has_api_key: false, policy: '', policy_keys: [], fail_open: true, timeout_seconds: null, temperature: null, max_tokens: null }, embedding: { enabled: false, endpoint: '', model: '', has_api_key: false, timeout_seconds: 10, fail_open: true, topic_cache_capacity: 10000, topic_cache_ttl_seconds: 3600, text_cache_capacity: 1000, text_cache_ttl_seconds: 60 }, circuit_breaker: { enabled: false, failure_threshold: 5, success_threshold: 2, recovery_timeout_seconds: 60, provider_overrides: [] }, background: { enabled: false, enable_group_flush: true, enable_timeout_processing: true, enable_approval_retry: true, enable_scheduled_actions: false, group_flush_interval_seconds: 5, timeout_check_interval_seconds: 10, cleanup_interval_seconds: 60, scheduled_check_interval_seconds: 5 }, telemetry: { enabled: false, endpoint: '', service_name: 'acteon', sample_ratio: 1.0, protocol: 'grpc', timeout_seconds: 10, resource_attribute_keys: [] }, chains: { max_concurrent_advances: 16, completed_chain_ttl_seconds: 604800, definitions: [] }, providers: [] }),
      }),
    )
    await page.goto('/settings/rate-limiting')
    await page.waitForTimeout(500)
    await expect(page.getByRole('heading', { name: 'Rate Limiting', exact: true })).toBeVisible()
  })

  test('event stream page renders at mobile viewport', async ({ page }) => {
    await page.goto('/stream')
    await expect(page.getByRole('heading', { name: 'Event Stream' })).toBeVisible()
  })

  test('dead-letter queue page renders at mobile viewport', async ({ page }) => {
    await page.goto('/dlq')
    await expect(page.getByRole('heading', { name: 'Dead-Letter Queue' })).toBeVisible()
  })
})
