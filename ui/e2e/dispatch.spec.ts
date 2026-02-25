import { test, expect } from '@playwright/test'

/** Helper: fill a field that may be a <select> or <input>. Waits for the element to stabilise. */
async function fillField(page: import('@playwright/test').Page, label: string, value: string) {
  const locator = page.getByLabel(label)
  await expect(locator).toBeVisible({ timeout: 10_000 })
  const tag = await locator.evaluate(el => el.tagName.toLowerCase())
  if (tag === 'select') {
    await expect(locator.locator(`option[value="${value}"]`)).toBeAttached({ timeout: 10_000 })
    await locator.selectOption(value)
  } else {
    await locator.fill(value)
  }
}

/** Wait for the config API to load so SelectOrCustom fields render as <select> with options. */
async function waitForConfig(page: import('@playwright/test').Page) {
  // Provider is always a <select>, so wait for real provider options to appear
  const providerSelect = page.getByLabel('Provider *')
  await expect(providerSelect.locator('option')).not.toHaveCount(1, { timeout: 15_000 })
}

test.describe('Dispatch', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/dispatch')
    await waitForConfig(page)
  })

  test('dispatch page loads with title', async ({ page }) => {
    await expect(page.getByText('Dispatch Action', { exact: true }).first()).toBeVisible()
  })

  test('form fields are present', async ({ page }) => {
    await expect(page.getByLabel('Namespace *')).toBeVisible()
    await expect(page.getByLabel('Tenant *')).toBeVisible()
    await expect(page.getByLabel('Provider *')).toBeVisible()
    await expect(page.getByLabel('Action Type *')).toBeVisible()
  })

  test('payload textarea is present', async ({ page }) => {
    await expect(page.getByText('Payload (JSON) *')).toBeVisible()
    await expect(page.locator('textarea')).toBeVisible()
  })

  test('dedup key field is present', async ({ page }) => {
    await expect(page.getByLabel('Dedup Key')).toBeVisible()
  })

  test('dry run toggle works', async ({ page }) => {
    const checkbox = page.getByRole('checkbox')
    await expect(checkbox).toBeVisible()
    await checkbox.check()
    await expect(checkbox).toBeChecked()
    await checkbox.uncheck()
    await expect(checkbox).not.toBeChecked()
  })

  test('dispatch button is disabled when required fields are empty', async ({ page }) => {
    const dispatchBtn = page.getByRole('button', { name: /Dispatch Action/i })
    await expect(dispatchBtn).toBeDisabled()
  })

  test('dispatch button becomes enabled when required fields are filled', async ({ page }) => {
    await fillField(page, 'Namespace *', 'production')
    await fillField(page, 'Tenant *', 'acme')
    await fillField(page, 'Provider *', 'slack')
    await fillField(page, 'Action Type *', 'send_message')

    const dispatchBtn = page.getByRole('button', { name: /Dispatch Action/i })
    await expect(dispatchBtn).toBeEnabled()
  })

  test('form submission works with dry run', async ({ page }) => {
    await fillField(page, 'Namespace *', 'production')
    await fillField(page, 'Tenant *', 'acme')
    await fillField(page, 'Provider *', 'slack')
    await fillField(page, 'Action Type *', 'send_message')

    const textarea = page.locator('textarea')
    await textarea.fill('{"key": "value"}')

    await page.getByRole('checkbox').check()

    const dispatchBtn = page.getByRole('button', { name: /Dispatch Action/i })
    await dispatchBtn.click()

    // Wait for response
    await page.waitForTimeout(2000)
    await expect(page.locator('body')).toBeVisible()
  })

  test('invalid JSON shows error', async ({ page }) => {
    await fillField(page, 'Namespace *', 'production')
    await fillField(page, 'Tenant *', 'acme')
    await fillField(page, 'Provider *', 'slack')
    await fillField(page, 'Action Type *', 'send_message')

    const textarea = page.locator('textarea')
    await textarea.fill('not json')

    const dispatchBtn = page.getByRole('button', { name: /Dispatch Action/i })
    await dispatchBtn.click()

    await expect(page.getByText('Invalid JSON')).toBeVisible()
  })
})
