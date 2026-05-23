/**
 * A2A protocol surface — factory helpers + shared constants.
 *
 * The {@link ActeonClient} class carries the actual HTTP methods
 * (kept inline because TypeScript doesn't have Python-style mixins
 * and the class already aggregates every other surface). This module
 * publishes the wire-shape factories that mirror the Python SDK's
 * `make_*` helpers, plus the `A2A-Version: 1.0` header constant.
 *
 * Wire payloads are typed as `Record<string, unknown>` — the A2A
 * schema is JSON-native and evolves; the factory functions hide the
 * camelCase conventions (`messageId`, `taskId`, `mediaType`) so
 * callers don't have to remember them.
 */

/** A2A protocol version this client speaks. Matches the Rust client's
 *  `A2A_PROTOCOL_VERSION` and the server's negotiated version. */
export const A2A_PROTOCOL_VERSION = "1.0";

/** HTTP header name carrying the negotiated A2A protocol version.
 *  Sent on every authenticated A2A call so the server's version
 *  negotiation honours a version-pinned caller. */
export const A2A_VERSION_HEADER = "A2A-Version";

/** Header object the client mixes into every authenticated request.
 *  Exported so callers writing their own integration can reuse the
 *  exact same constant. */
export const A2A_HEADERS: Readonly<Record<string, string>> = Object.freeze({
  [A2A_VERSION_HEADER]: A2A_PROTOCOL_VERSION,
});

/** Build a text `Part` payload — the lightest A2A part shape.
 *  The server rejects text larger than 256 KiB
 *  (`MAX_PART_TEXT_BYTES`) at validation time. */
export function makePartText(text: string): Record<string, unknown> {
  return { text };
}

/** Build a URL-reference `Part`. Use this for payloads that exceed
 *  the 256 KiB inline cap — the URL points at an external store the
 *  receiver fetches separately. */
export function makePartUrl(href: string): Record<string, unknown> {
  return { url: href };
}

/** Build a structured-data `Part`. The server JSON-encodes `value`
 *  to measure against `MAX_PART_DATA_BYTES = 256 KiB`. `mediaType`
 *  defaults to `application/json`. */
export function makePartData(
  value: unknown,
  mediaType: string = "application/json",
): Record<string, unknown> {
  return { data: value, mediaType };
}

/** Options accepted by {@link makeMessage}. */
export interface MakeMessageOptions {
  /** Thread the message into an existing Task's history. Omit to
   *  mint a fresh Task on `a2aSendMessage`. */
  taskId?: string;
  /** Optional context id (carried across related tasks). */
  contextId?: string;
}

/** Build a `TaskMessage` payload. `role` must be `"user"` or
 *  `"agent"` — the server validates. */
export function makeMessage(
  messageId: string,
  role: "user" | "agent",
  parts: Record<string, unknown>[],
  options: MakeMessageOptions = {},
): Record<string, unknown> {
  const msg: Record<string, unknown> = { messageId, role, parts };
  if (options.taskId !== undefined) msg.taskId = options.taskId;
  if (options.contextId !== undefined) msg.contextId = options.contextId;
  return msg;
}

/** Options accepted by {@link makePushConfig}. */
export interface MakePushConfigOptions {
  /** Pre-allocate the config id (UUIDv7 by convention). Omit to let
   *  the server mint one. */
  id?: string;
  /** Optional bearer token sent in
   *  `Authorization: Bearer <token>` on every push POST. */
  token?: string;
  /** Optional richer authentication metadata. */
  authentication?: Record<string, unknown>;
}

/** Build a `PushNotificationConfig` body. `url` must be `http://` or
 *  `https://` — the server denies other schemes at registration
 *  time. */
export function makePushConfig(
  url: string,
  options: MakePushConfigOptions = {},
): Record<string, unknown> {
  const body: Record<string, unknown> = { url };
  if (options.id !== undefined) body.id = options.id;
  if (options.token !== undefined) body.token = options.token;
  if (options.authentication !== undefined) {
    body.authentication = options.authentication;
  }
  return body;
}

/**
 * Percent-encode a path segment opaquely (no `/` passthrough).
 * Mirrors the segment encoding used by the Python and Rust clients
 * so a tenant id with reserved characters cannot leak into
 * additional path components.
 */
export function a2aSegment(s: string): string {
  return encodeURIComponent(s);
}

/** JSON-RPC 2.0 envelope shape used for the one
 *  `agent/getAuthenticatedExtendedCard` round-trip. */
export interface JsonRpcReply<T> {
  jsonrpc: "2.0";
  id: number | string | null;
  result?: T;
  error?: { code: number; message: string; data?: unknown };
}

/**
 * Unwrap a JSON-RPC 2.0 reply envelope. Throws an `ApiError`-shaped
 * `Error` when the envelope carries an `error` member.
 */
export function unwrapJsonRpc<T>(
  body: JsonRpcReply<T>,
  raise: (code: string, message: string) => never,
): T {
  if (body.error) {
    raise(String(body.error.code), body.error.message);
  }
  if (body.result === undefined) {
    raise("JSONRPC", "JSON-RPC reply had neither result nor error");
  }
  return body.result as T;
}
