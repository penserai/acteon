use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use tower::{Layer, Service};

use crate::auth::identity::CallerIdentity;

use super::limiter::{RateLimitResult, RateLimiter, ANONYMOUS_BUCKET};

/// Tower layer that adds rate limiting middleware.
#[derive(Clone)]
pub struct RateLimitLayer {
    limiter: Option<Arc<RateLimiter>>,
}

impl RateLimitLayer {
    pub fn new(limiter: Option<Arc<RateLimiter>>) -> Self {
        Self { limiter }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitMiddleware {
            inner,
            limiter: self.limiter.clone(),
        }
    }
}

/// Tower service that enforces rate limits on requests.
#[derive(Clone)]
pub struct RateLimitMiddleware<S> {
    inner: S,
    limiter: Option<Arc<RateLimiter>>,
}

impl<S> Service<Request<Body>> for RateLimitMiddleware<S>
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

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let limiter = self.limiter.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            let Some(limiter) = limiter else {
                // Rate limiting disabled: pass through.
                return inner.call(req).await;
            };

            // Get caller ID from request extensions (set by AuthMiddleware).
            let caller_id = req
                .extensions()
                .get::<CallerIdentity>()
                .map_or_else(
                    || ANONYMOUS_BUCKET.to_owned(),
                    |id| {
                        if id.id.is_empty() {
                            ANONYMOUS_BUCKET.to_owned()
                        } else {
                            id.id.clone()
                        }
                    },
                );

            // Check rate limit for the caller.
            match limiter.check_caller_limit(&caller_id).await {
                Ok(result) => {
                    // Allowed: proceed with request, then add rate limit headers to response.
                    let response = inner.call(req).await?;
                    Ok(add_rate_limit_headers(response, &result))
                }
                Err(exceeded) => {
                    // Rate limited: return 429 response.
                    Ok(rate_limited_response(exceeded.retry_after, exceeded.limit))
                }
            }
        })
    }
}

/// Add rate limit headers to a successful response.
fn add_rate_limit_headers(response: Response, result: &RateLimitResult) -> Response {
    let (mut parts, body) = response.into_parts();

    parts
        .headers
        .insert("X-RateLimit-Limit", result.limit.into());
    parts
        .headers
        .insert("X-RateLimit-Remaining", result.remaining.into());
    parts.headers.insert(
        "X-RateLimit-Reset",
        result.reset_after.into(),
    );

    Response::from_parts(parts, body)
}

/// Build a 429 Too Many Requests response.
fn rate_limited_response(retry_after: u64, limit: u64) -> Response {
    let body = serde_json::json!({
        "error": "rate limit exceeded",
        "retry_after": retry_after,
        "limit": limit
    });

    let mut response = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();

    response.headers_mut().insert(
        header::RETRY_AFTER,
        retry_after.into(),
    );
    response
        .headers_mut()
        .insert("X-RateLimit-Limit", limit.into());
    response
        .headers_mut()
        .insert("X-RateLimit-Remaining", 0u64.into());

    response
}
