/** Mock identity exchange for helpers validating server-assigned ownership. */
export function credentialFetch(handler: (input: string, init: RequestInit) => Promise<Response>): (input: string, init: RequestInit) => Promise<Response> {
  return ((input: string, init: RequestInit) => {
    if (new URL(String(input)).pathname === "/v1/auth/identity") {
      return Promise.resolve(new Response(JSON.stringify({
        credential_id: "test-key", auth_method: "api_key", role: "operator", principal: null,
        scope: { namespace: "prod", tenant: "acme" },
      })));
    }
    return handler(input, init);
  });
}
