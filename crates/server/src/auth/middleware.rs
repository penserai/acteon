use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use tower::{Layer, Service};

use super::AuthProvider;
use super::identity::CallerIdentity;

/// Tower layer that adds authentication middleware.
#[derive(Clone)]
pub struct AuthLayer {
    provider: Option<Arc<AuthProvider>>,
}

impl AuthLayer {
    pub fn new(provider: Option<Arc<AuthProvider>>) -> Self {
        Self { provider }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthMiddleware {
            inner,
            provider: self.provider.clone(),
        }
    }
}

/// Tower service that authenticates requests.
#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    provider: Option<Arc<AuthProvider>>,
}

impl<S> Service<Request<Body>> for AuthMiddleware<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        let provider = self.provider.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            let Some(provider) = provider else {
                // Auth disabled: inject anonymous identity with full access.
                req.extensions_mut().insert(CallerIdentity::anonymous());
                return inner.call(req).await;
            };

            // Try the Authorization: Bearer header first. JWT validation
            // runs first; if it fails (e.g., the caller is using a raw
            // API key instead of a JWT), fall back to treating the token
            // as an API key. This makes API-key auth work for SDKs that
            // send credentials via the standard Authorization header.
            let bearer_token = req
                .headers()
                .get("authorization")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "))
                .map(str::to_owned);

            if let Some(ref token) = bearer_token {
                match provider.validate_jwt(token).await {
                    Ok(identity) => {
                        req.extensions_mut().insert(identity);
                        return inner.call(req).await;
                    }
                    Err(jwt_err) => {
                        // JWT validation failed — try API key fallback
                        // before returning 401.
                        if let Some(identity) = provider.authenticate_api_key(token).await {
                            req.extensions_mut().insert(identity);
                            return inner.call(req).await;
                        }
                        // Neither JWT nor API key worked. Surface the
                        // JWT error so Bearer-JWT callers see a useful
                        // message (invalid signature, expired, etc.).
                        return Ok(unauthorized(&jwt_err));
                    }
                }
            }

            // Legacy explicit X-API-Key header path. Still supported for
            // tools and curl examples that set the dedicated header.
            if let Some(api_key_header) = req.headers().get("x-api-key")
                && let Ok(key_str) = api_key_header.to_str()
            {
                match provider.authenticate_api_key(key_str).await {
                    Some(identity) => {
                        req.extensions_mut().insert(identity);
                        return inner.call(req).await;
                    }
                    None => {
                        return Ok(unauthorized("invalid API key"));
                    }
                }
            }

            Ok(unauthorized("missing authentication credentials"))
        })
    }
}

fn unauthorized(message: &str) -> Response {
    let body = serde_json::json!({ "error": message });
    (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response()
}
