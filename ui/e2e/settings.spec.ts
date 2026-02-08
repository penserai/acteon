import { test, expect } from '@playwright/test'

const configFixture = {
  server: { host: '127.0.0.1', port: 8080, shutdown_timeout_seconds: 30, external_url: null, max_sse_connections_per_tenant: null },
  state: { backend: 'memory', has_url: false, prefix: null, region: null, table_name: null },
  executor: { max_retries: null, timeout_seconds: null, max_concurrent: null, dlq_enabled: false },
  rules: { directory: null, default_timezone: null },
  audit: { enabled: false, backend: 'memory', has_url: false, prefix: '', ttl_seconds: 2592000, cleanup_interval_seconds: 3600, store_payload: true, redact: { enabled: false, field_count: 0, placeholder: '[REDACTED]' } },
  auth: { enabled: false, config_path: null, watch: null },
  rate_limit: { enabled: false, config_path: null, on_error: 'allow' },
  llm_guardrail: { enabled: false, endpoint: 'https://api.openai.com/v1/chat/completions', model: 'gpt-4o-mini', has_api_key: false, policy: '', policy_keys: [], fail_open: true, timeout_seconds: null, temperature: null, max_tokens: null },
  embedding: { enabled: false, endpoint: 'https://api.openai.com/v1/embeddings', model: 'text-embedding-3-small', has_api_key: false, timeout_seconds: 10, fail_open: true, topic_cache_capacity: 10000, topic_cache_ttl_seconds: 3600, text_cache_capacity: 1000, text_cache_ttl_seconds: 60 },
  circuit_breaker: { enabled: false, failure_threshold: 5, success_threshold: 2, recovery_timeout_seconds: 60, provider_overrides: [] },
  background: { enabled: false, enable_group_flush: true, enable_timeout_processing: true, enable_approval_retry: true, enable_scheduled_actions: false, group_flush_interval_seconds: 5, timeout_check_interval_seconds: 10, cleanup_interval_seconds: 60, scheduled_check_interval_seconds: 5 },
  telemetry: { enabled: false, endpoint: 'http://localhost:4317', service_name: 'acteon', sample_ratio: 1.0, protocol: 'grpc', timeout_seconds: 10, resource_attribute_keys: [] },
  chains: { max_concurrent_advances: 16, completed_chain_ttl_seconds: 604800, definitions: [] },
  providers: [],
}

test.describe('Settings', () => {
  test.beforeEach(async ({ page }) => {
    await page.route('**/admin/config', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(configFixture),
      }),
    )
    await page.route('**/health', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ status: 'ok', metrics: { dispatched: 0, executed: 0, deduplicated: 0, suppressed: 0, rerouted: 0, throttled: 0, failed: 0, llm_guardrail_allowed: 0, llm_guardrail_denied: 0, llm_guardrail_errors: 0, chains_started: 0, chains_completed: 0, chains_failed: 0, chains_cancelled: 0 } }),
      }),
    )
  })

  test('default settings route redirects to server config', async ({ page }) => {
    await page.goto('/settings')
    await page.waitForURL('**/settings/config')
    await expect(page.getByRole('heading', { name: 'Server Config' })).toBeVisible()
  })

  test('server config page shows configuration cards', async ({ page }) => {
    await page.goto('/settings/config')
    await page.waitForTimeout(500)
    await expect(page.getByRole('heading', { name: 'Server Config' })).toBeVisible()
    await expect(page.getByText('127.0.0.1')).toBeVisible()
    await expect(page.getByText('8080')).toBeVisible()
  })

  test('theme toggle shows three options', async ({ page }) => {
    await page.goto('/settings/config')
    await page.waitForTimeout(500)
    await expect(page.getByRole('button', { name: 'System', exact: true })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Light', exact: true })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Dark', exact: true })).toBeVisible()
  })

  test('theme toggle switches between modes', async ({ page }) => {
    const viewport = page.viewportSize()
    test.skip(!!viewport && viewport.width < 768, 'Theme buttons obscured on narrow viewports')

    await page.goto('/settings/config')
    await page.waitForTimeout(500)

    await page.getByRole('button', { name: 'Dark', exact: true }).click()
    await expect(page.locator('html')).toHaveClass(/dark/)

    await page.getByRole('button', { name: 'Light', exact: true }).click()
    await expect(page.locator('html')).not.toHaveClass(/dark/)
  })

  test('theme persists after page reload', async ({ page }) => {
    const viewport = page.viewportSize()
    test.skip(!!viewport && viewport.width < 768, 'Theme buttons obscured on narrow viewports')

    await page.goto('/settings/config')
    await page.waitForTimeout(500)

    await page.getByRole('button', { name: 'Dark', exact: true }).click()
    await expect(page.locator('html')).toHaveClass(/dark/)

    await page.reload()
    await page.waitForTimeout(500)
    await expect(page.locator('html')).toHaveClass(/dark/)

    // Restore to system
    await page.getByRole('button', { name: 'System', exact: true }).click()
  })

  test('rate limiting page loads', async ({ page }) => {
    await page.goto('/settings/rate-limiting')
    await page.waitForTimeout(500)
    await expect(page.getByRole('heading', { name: 'Rate Limiting', exact: true })).toBeVisible()
  })

  test('auth page loads', async ({ page }) => {
    await page.goto('/settings/auth')
    await page.waitForTimeout(500)
    await expect(page.getByRole('heading', { name: 'Auth & Users', exact: true })).toBeVisible()
  })

  test('providers settings page loads', async ({ page }) => {
    await page.goto('/settings/providers')
    await page.waitForTimeout(500)
    await expect(page.getByRole('heading', { name: 'Providers', exact: true })).toBeVisible()
  })

  test('llm guardrail page loads', async ({ page }) => {
    await page.goto('/settings/llm')
    await page.waitForTimeout(500)
    await expect(page.getByRole('heading', { name: 'LLM Guardrail', exact: true })).toBeVisible()
  })

  test('telemetry page loads', async ({ page }) => {
    await page.goto('/settings/telemetry')
    await page.waitForTimeout(500)
    await expect(page.getByRole('heading', { name: 'Telemetry', exact: true })).toBeVisible()
  })

  test('background tasks page loads', async ({ page }) => {
    await page.goto('/settings/background')
    await page.waitForTimeout(500)
    await expect(page.getByRole('heading', { name: 'Background Tasks', exact: true })).toBeVisible()
  })
})
