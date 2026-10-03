/** Finite HTTP operations preserve server wire names and envelopes. */
import { platformOperations, type PlatformOperation } from "./platform_catalog.js";
export { platformOperations, type PlatformOperation } from "./platform_catalog.js";

export interface PlatformRequestOptions {
  path?: Record<string, string>;
  query?: Record<string, string | string[]>;
  body?: unknown;
}

export function platformRequestParts(operation: PlatformOperation, options: PlatformRequestOptions) {
  const descriptor = platformOperations[operation];
  if (!descriptor) throw new Error("Unknown platform operation");
  const values = options.path ?? {};
  const expected: readonly string[] = descriptor.parameters;
  if (Object.keys(values).length !== expected.length || expected.some(key => !(key in values))) {
    throw new Error(`Expected path parameters: ${expected.join(", ")}`);
  }
  let path: string = descriptor.path;
  for (const [name, value] of Object.entries(values)) {
    if (!value || value === "." || value === "..") throw new Error("Empty and dot path segments are not allowed");
    path = path.replace(`{${name}}`, encodeURIComponent(value));
  }
  if (descriptor.method === "GET" && options.body !== undefined) throw new Error("GET operations do not accept a body");
  const params = new URLSearchParams();
  for (const [name, value] of Object.entries(options.query ?? {})) {
    for (const item of Array.isArray(value) ? value : [value]) params.append(name, item);
  }
  return { ...descriptor, path, params };
}
