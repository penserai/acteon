package acteon

import (
	"io"
	"net/http"
	"strings"
)

const testCredentialIdentity = `{"credential_id":"test-key","auth_method":"api_key","role":"operator","principal":null,"scope":{"namespace":"prod","tenant":"acme"}}`

func testIdentityResponse(request *http.Request) *http.Response {
	if request.URL.Path != "/v1/auth/identity" {
		return nil
	}
	return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": []string{"application/json"}}, Body: io.NopCloser(strings.NewReader(testCredentialIdentity)), Request: request}
}

func withTestIdentity(handler http.HandlerFunc) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/v1/auth/identity" {
			w.Header().Set("Content-Type", "application/json")
			io.WriteString(w, testCredentialIdentity)
			return
		}
		handler(w, r)
	})
}
