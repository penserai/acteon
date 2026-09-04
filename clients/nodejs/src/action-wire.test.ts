import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import { actionToRequest, createAction } from "./models.js";

it("serializes metadata using the Rust wire fixture", () => {
  const fixture = JSON.parse(readFileSync(new URL("../../contract-fixtures/action-wire.json", import.meta.url), "utf8"));
  const action = createAction(fixture.namespace, fixture.tenant, fixture.provider, fixture.action_type, fixture.payload);
  action.id = fixture.id;
  action.createdAt = new Date(fixture.created_at).toISOString();
  action.metadata = fixture.metadata;
  const request = actionToRequest(action);
  // JavaScript emits milliseconds; both represent the same RFC3339 instant.
  fixture.created_at = new Date(fixture.created_at).toISOString();
  expect(request).toEqual(fixture);
});
