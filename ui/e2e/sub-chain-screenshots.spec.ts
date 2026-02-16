/**
 * Screenshot script for sub-chains documentation.
 *
 * Prerequisites:
 *   1. Start the server: cargo run -p acteon-server -- --config examples/sub-chains.toml
 *   2. Dispatch chains via MCP or curl (incident_detected, deploy_requested, user_signup)
 *   3. Run: cd ui && npx playwright test e2e/sub-chain-screenshots.spec.ts --project chromium
 *
 * Output: docs/screenshots/sub-chains-*.png
 */
import { test } from '@playwright/test'

const SCREENSHOT_DIR = '../docs/screenshots'

// Only run in chromium (skip mobile-chrome) for documentation screenshots
test.skip(({ browserName }) => browserName !== 'chromium')

test.describe('Sub-chain screenshots', () => {
  test('chain list with sub-chains', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 })
    await page.goto('/chains')

    await page.getByPlaceholder('Namespace').fill('ops')
    await page.getByPlaceholder('Tenant').fill('acme-corp')
    await page.getByPlaceholder('Tenant').press('Enter')
    await page.waitForTimeout(1500)

    await page.screenshot({
      path: `${SCREENSHOT_DIR}/sub-chains-list.png`,
      fullPage: false,
    })
  })

  test('incident-response chain detail with expanded DAG', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 })
    await page.goto('/chains')

    await page.getByPlaceholder('Namespace').fill('ops')
    await page.getByPlaceholder('Tenant').fill('acme-corp')
    await page.getByPlaceholder('Tenant').press('Enter')
    await page.waitForTimeout(1500)

    const incidentRow = page.locator('tr', { hasText: 'incident-response' }).first()
    if (await incidentRow.isVisible()) {
      await incidentRow.click()
      await page.waitForURL(/\/chains\//)
      // Append expand=all to the current URL to expand all sub-chain nodes
      const url = new URL(page.url())
      url.searchParams.set('expand', 'all')
      await page.goto(url.toString())
      await page.waitForTimeout(2500)

      await page.screenshot({
        path: `${SCREENSHOT_DIR}/sub-chains-detail.png`,
        fullPage: true,
      })
    }
  })

  test('deploy-pipeline chain detail with expanded sub-chains', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 })
    await page.goto('/chains')

    await page.getByPlaceholder('Namespace').fill('ops')
    await page.getByPlaceholder('Tenant').fill('acme-corp')
    await page.getByPlaceholder('Tenant').press('Enter')
    await page.waitForTimeout(1500)

    const deployRow = page.locator('tr', { hasText: 'deploy-pipeline' }).first()
    if (await deployRow.isVisible()) {
      await deployRow.click()
      await page.waitForURL(/\/chains\//)
      // Append expand=all to the current URL to expand all sub-chain nodes
      const url = new URL(page.url())
      url.searchParams.set('expand', 'all')
      await page.goto(url.toString())
      await page.waitForTimeout(2500)

      await page.screenshot({
        path: `${SCREENSHOT_DIR}/sub-chains-deploy-pipeline.png`,
        fullPage: true,
      })
    }
  })

  test('DAG API response for incident-response (pretty)', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 })

    // Fetch the JSON and display it nicely
    const response = await page.request.get('/v1/chains/definitions/incident-response/dag')
    const json = await response.json()
    const prettyJson = JSON.stringify(json, null, 2)

    // Create a page with syntax-highlighted JSON
    await page.setContent(`
      <html>
        <head>
          <style>
            body {
              background: #1e1e2e;
              color: #cdd6f4;
              font-family: 'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace;
              font-size: 12px;
              padding: 24px;
              margin: 0;
              line-height: 1.5;
            }
            .header {
              color: #89b4fa;
              font-size: 14px;
              font-weight: bold;
              margin-bottom: 16px;
              padding-bottom: 8px;
              border-bottom: 1px solid #45475a;
            }
            pre {
              white-space: pre-wrap;
              word-break: break-word;
            }
            .string { color: #a6e3a1; }
            .number { color: #fab387; }
            .boolean { color: #f38ba8; }
            .null { color: #6c7086; }
            .key { color: #89b4fa; }
            .bracket { color: #cdd6f4; }
          </style>
        </head>
        <body>
          <div class="header">GET /v1/chains/definitions/incident-response/dag</div>
          <pre>${syntaxHighlight(prettyJson)}</pre>
        </body>
      </html>
    `)

    await page.waitForTimeout(300)
    await page.screenshot({
      path: `${SCREENSHOT_DIR}/sub-chains-dag-api.png`,
      fullPage: false,
    })
  })

  test('rules page showing chain triggers', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 })
    await page.goto('/rules')
    await page.waitForTimeout(1500)

    await page.screenshot({
      path: `${SCREENSHOT_DIR}/sub-chains-rules.png`,
      fullPage: false,
    })
  })
})

/** Simple JSON syntax highlighter for screenshot. */
function syntaxHighlight(json: string): string {
  return json
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(
      /("(\\u[\da-fA-F]{4}|\\[^u]|[^\\"])*"(\s*:)?|\b(true|false|null)\b|-?\d+(?:\.\d*)?(?:[eE][+-]?\d+)?)/g,
      (match) => {
        let cls = 'number'
        if (/^"/.test(match)) {
          cls = /:$/.test(match) ? 'key' : 'string'
        } else if (/true|false/.test(match)) {
          cls = 'boolean'
        } else if (/null/.test(match)) {
          cls = 'null'
        }
        return `<span class="${cls}">${match}</span>`
      },
    )
}
