//! Checked endpoint permission inventory. This layer is a role ceiling;
//! handlers remain responsible for resource/grant authorization.
use std::sync::LazyLock;

use axum::Json;
use axum::extract::{MatchedPath, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::identity::CallerIdentity;
use super::role::Permission;

#[derive(Debug, Deserialize)]
pub struct RoutePermission {
    pub name: String,
    pub method: String,
    pub path: String,
    /// Public endpoints have their own authentication contract.
    pub permission: Option<Permission>,
}

pub static ROUTES: LazyLock<Vec<RoutePermission>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("route_permissions.json"))
        .expect("checked route permission inventory must be valid")
});

/// Runs after authentication and before body extraction or handler effects.
/// Missing entries fail closed, including newly registered routes.
pub async fn authorize_route(request: Request, next: Next) -> Response {
    let Some(identity) = request.extensions().get::<CallerIdentity>() else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "missing authenticated identity"
            })),
        )
            .into_response();
    };
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str);
    let method = if request.method() == "HEAD" {
        "GET"
    } else {
        request.method().as_str()
    };
    let permission = ROUTES
        .iter()
        .find(|route| Some(route.path.as_str()) == path && route.method == method)
        .and_then(|route| route.permission);
    let Some(permission) = permission else {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "endpoint has no registered permission"
            })),
        )
            .into_response();
    };
    if !identity.role.has_permission(permission) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!("role {} lacks {permission:?} permission", identity.role)
            })),
        )
            .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Extension, Router, middleware, routing::post};
    use tower::ServiceExt;

    #[tokio::test]
    async fn unclassified_new_route_is_denied_even_to_admin() {
        let router = Router::new()
            .route("/v1/unclassified", post(|| async { "unexpected effect" }))
            .route_layer(middleware::from_fn(authorize_route))
            .layer(Extension(CallerIdentity::anonymous()));
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/unclassified")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn agent_continuation_routes_require_dispatch_permission() {
        for path in [
            "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/message:send",
            "/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}/message:send",
        ] {
            let route = ROUTES
                .iter()
                .find(|route| route.method == "POST" && route.path == path)
                .unwrap_or_else(|| panic!("missing permission inventory entry for {path}"));
            assert_eq!(route.permission, Some(Permission::Dispatch));
        }
    }
}
