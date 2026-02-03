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

            // Try Bearer token first.
            if let Some(auth_header) = req.headers().get("authorization")
                && let Ok(header_str) = auth_header.to_str()
                && let Some(token) = header_str.strip_prefix("Bearer ")
            {
                match provider.validate_jwt(token).await {
                    Ok(identity) => {
                        req.extensions_mut().insert(identity);
                        return inner.call(req).await;
                    }
                    Err(e) => {
                        return Ok(unauthorized(&e));
                    }
                }
            }

            // Try API key.
            if let Some(api_key_header) = req.headers().get("x-api-key")
                && let Ok(key_str) = api_key_header.to_str()
            {
                match provider.authenticate_api_key(key_str) {
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
