#!/usr/bin/env node
/**
 * Test script for the Node.js Acteon client.
 *
 * Usage:
 *   ACTEON_URL=http://localhost:8080 node test_nodejs_client.mjs
 */

import { randomUUID } from "crypto";

// Import from built client (must run npm build first)
const clientPath = new URL("../../clients/nodejs/dist/index.js", import.meta.url);
const { ActeonClient, createAction } = await import(clientPath);

const baseUrl = process.env.ACTEON_URL || "http://localhost:8080";

console.log(`Node.js Client Test - connecting to ${baseUrl}`);
console.log("=".repeat(60));

const client = new ActeonClient(baseUrl);
const results = { passed: 0, failed: 0 };

async function test(name, fn) {
  try {
    await fn();
    console.log(`  [PASS] ${name}`);
    results.passed++;
  } catch (e) {
    console.log(`  [FAIL] ${name}: ${e.message}`);
    results.failed++;
  }
}

// Test: Health check
await test("health()", async () => {
  const healthy = await client.health();
  if (!healthy) throw new Error("Health check failed");
});

// Test: Single dispatch
let dispatchedId = null;
await test("dispatch()", async () => {
  const action = createAction(
    "test",
    "nodejs-client",
    "email",
    "send_notification",
    { to: "test@example.com", subject: "Node.js test" }
  );
  dispatchedId = action.id;
  const outcome = await client.dispatch(action);
  const validTypes = [
    "executed",
    "deduplicated",
    "suppressed",
    "rerouted",
    "throttled",
    "failed",
  ];
  if (!validTypes.includes(outcome.type)) {
    throw new Error(`Unexpected outcome: ${outcome.type}`);
  }
});

// Test: Batch dispatch
await test("dispatchBatch()", async () => {
  const actions = [0, 1, 2].map((i) =>
    createAction("test", "nodejs-client", "email", "batch_test", { seq: i })
  );
  const resultsList = await client.dispatchBatch(actions);
  if (resultsList.length !== 3) {
    throw new Error(`Expected 3 results, got ${resultsList.length}`);
  }
});

// Test: List rules
await test("listRules()", async () => {
  const rules = await client.listRules();
  if (!Array.isArray(rules)) {
    throw new Error("Expected array of rules");
  }
});

// Test: Deduplication
await test("deduplication", async () => {
  const dedupKey = `nodejs-dedup-${randomUUID()}`;
  const action1 = createAction(
    "test",
    "nodejs-client",
    "email",
    "dedup_test",
    { msg: "first" },
    { dedupKey }
  );
  const action2 = createAction(
    "test",
    "nodejs-client",
    "email",
    "dedup_test",
    { msg: "second" },
    { dedupKey }
  );
  const outcome1 = await client.dispatch(action1);
  const outcome2 = await client.dispatch(action2);
  // First should execute, second may be deduplicated
  if (!["executed", "failed"].includes(outcome1.type)) {
    throw new Error(`Unexpected first outcome: ${outcome1.type}`);
  }
});

// Test: Query audit
await test("queryAudit()", async () => {
  const page = await client.queryAudit({ tenant: "nodejs-client", limit: 10 });
  if (typeof page.total !== "number") {
    throw new Error("Expected AuditPage with total");
  }
  if (!Array.isArray(page.records)) {
    throw new Error("Expected AuditPage with records");
  }
});

// Summary
console.log("=".repeat(60));
const total = results.passed + results.failed;
console.log(`Results: ${results.passed}/${total} passed`);

process.exit(results.failed === 0 ? 0 : 1);
