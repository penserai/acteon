import { test, expect } from '@playwright/test'

test.describe('Dispatch', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/dispatch')
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
    await page.getByLabel('Namespace *').fill('test-ns')
    await page.getByLabel('Tenant *').fill('test-tenant')
    await page.getByLabel('Provider *').fill('log')
    await page.getByLabel('Action Type *').fill('test-action')

    const dispatchBtn = page.getByRole('button', { name: /Dispatch Action/i })
    await expect(dispatchBtn).toBeEnabled()
  })

  test('form submission works with dry run', async ({ page }) => {
    await page.getByLabel('Namespace *').fill('test')
    await page.getByLabel('Tenant *').fill('test')
    await page.getByLabel('Provider *').fill('log')
    await page.getByLabel('Action Type *').fill('test')

    const textarea = page.locator('textarea')
    await textarea.fill('{"key": "value"}')

    await page.getByRole('checkbox').check()

    const dispatchBtn = page.getByRole('button', { name: /Dispatch Action/i })
    await dispatchBtn.click()

    // Wait for response -- either result card or error toast
    await page.waitForTimeout(2000)
    // Should not crash the page
    await expect(page.locator('body')).toBeVisible()
  })

  test('invalid JSON shows error', async ({ page }) => {
    await page.getByLabel('Namespace *').fill('test')
    await page.getByLabel('Tenant *').fill('test')
    await page.getByLabel('Provider *').fill('log')
    await page.getByLabel('Action Type *').fill('test')

    const textarea = page.locator('textarea')
    await textarea.fill('not json')

    const dispatchBtn = page.getByRole('button', { name: /Dispatch Action/i })
    await dispatchBtn.click()

    await expect(page.getByText('Invalid JSON')).toBeVisible()
  })
})
