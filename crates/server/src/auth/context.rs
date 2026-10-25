//! Bind HTTP data models to authenticated tenant context before deserialization.
//!
//! Scope fields are infrastructure metadata on internal records, not request
//! inputs. Only declared context locations are populated: arbitrary payload and
//! attribute objects are never traversed or rewritten.
// Axum extractors return HTTP responses directly as their rejection type.
#![allow(clippy::result_large_err)]

use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Query, Request},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::identity::CallerIdentity;
use acteon_core::TenantContext;

fn rejection(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({"error": message}))).into_response()
}

fn authenticated_scope(parts: &Parts) -> Result<TenantContext, Response> {
    parts
        .extensions
        .get::<CallerIdentity>()
        .map(|identity| identity.scope.clone())
        .ok_or_else(|| {
            rejection(
                StatusCode::UNAUTHORIZED,
                "authenticated tenant context required",
            )
        })
}

/// Extract the exact context authenticated by JWT or API key.
pub struct RequestScope(pub TenantContext);

impl<S: Send + Sync> FromRequestParts<S> for RequestScope {
    type Rejection = Response;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(
            reject_scope_query(parts.uri.query())
                .and_then(|()| authenticated_scope(parts).map(Self)),
        )
    }
}

pub(super) fn reject_scope_query(query: Option<&str>) -> Result<(), Response> {
    if url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()).any(|(key, _)| {
        matches!(
            key.as_ref(),
            "namespace" | "tenant" | "scope" | "tenant_scope"
        )
    }) {
        return Err(rejection(
            StatusCode::BAD_REQUEST,
            "tenant and namespace are derived from the credential; request scope is not accepted",
        ));
    }
    Ok(())
}

/// Explicit opt-in for a JSON model whose infrastructure context is populated
/// before its ordinary deserializer runs. Implementations must not inspect
/// arbitrary user data to discover scope locations.
pub trait ScopedBody: DeserializeOwned {
    fn bind_context(body: &mut Value, scope: &TenantContext) -> Result<(), Response>;
}

/// Populate root context fields on a known internal request model.
pub fn bind_root(body: &mut Value, scope: &TenantContext) -> Result<(), Response> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| rejection(StatusCode::BAD_REQUEST, "expected a JSON object"))?;
    if ["namespace", "tenant", "scope", "tenant_scope"]
        .iter()
        .any(|key| object.contains_key(*key))
    {
        return Err(rejection(
            StatusCode::BAD_REQUEST,
            "tenant and namespace are derived from the credential; request scope is not accepted",
        ));
    }
    object.insert("namespace".into(), Value::String(scope.namespace.clone()));
    object.insert("tenant".into(), Value::String(scope.tenant.clone()));
    Ok(())
}

/// Populate a declared nested context envelope (for example timeline's internal
/// scope). The envelope itself is not accepted from the HTTP caller.
pub fn bind_envelope(body: &mut Value, scope: &TenantContext) -> Result<(), Response> {
    bind_root(body, scope)?;
    let object = body.as_object_mut().expect("root binding checked object");
    object.remove("namespace");
    object.remove("tenant");
    object.insert("scope".into(), serde_json::json!(scope));
    Ok(())
}

impl<T: ScopedBody> ScopedBody for Vec<T> {
    fn bind_context(body: &mut Value, scope: &TenantContext) -> Result<(), Response> {
        let values = body
            .as_array_mut()
            .ok_or_else(|| rejection(StatusCode::BAD_REQUEST, "expected a JSON array"))?;
        for value in values {
            T::bind_context(value, scope)?;
        }
        Ok(())
    }
}

/// JSON input with infrastructure scope supplied exclusively by authentication.
pub struct ScopedJson<T>(pub T);

// Domain records retain ownership fields for storage and responses. Their HTTP
// input schema omits that infrastructure metadata just as the extractor does.
impl<T: utoipa::ToSchema> utoipa::__dev::ComposeSchema for ScopedJson<T> {
    fn compose(
        _generics: Vec<utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>>,
    ) -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::{RefOr, schema::Schema};
        let mut schema = T::schema();
        if let RefOr::T(Schema::Object(object)) = &mut schema {
            for key in ["namespace", "tenant", "scope", "tenant_scope"] {
                object.properties.remove(key);
                object.required.retain(|field| field != key);
            }
        }
        schema
    }
}

impl<T: utoipa::ToSchema> utoipa::ToSchema for ScopedJson<T> {
    fn name() -> std::borrow::Cow<'static, str> {
        format!("CredentialInput_{}", T::name()).into()
    }

    fn schemas(
        schemas: &mut Vec<(
            String,
            utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>,
        )>,
    ) {
        T::schemas(schemas);
    }
}

impl<S: Send + Sync, T: ScopedBody> FromRequest<S> for ScopedJson<T> {
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        reject_scope_query(parts.uri.query())?;
        let scope = authenticated_scope(&parts)?;
        let Json(mut value) = Json::<Value>::from_request(Request::from_parts(parts, body), state)
            .await
            .map_err(IntoResponse::into_response)?;
        T::bind_context(&mut value, &scope)?;
        serde_json::from_value(value)
            .map(Self)
            .map_err(|e| rejection(StatusCode::UNPROCESSABLE_ENTITY, &e.to_string()))
    }
}

/// Query input with exact context supplied exclusively by authentication.
pub struct ScopedQuery<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for ScopedQuery<T> {
    type Rejection = Response;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(scoped_query(parts).map(Self))
    }
}

fn scoped_query<T: DeserializeOwned>(parts: &Parts) -> Result<T, Response> {
    reject_scope_query(parts.uri.query())?;
    let scope = authenticated_scope(parts)?;
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.extend_pairs(url::form_urlencoded::parse(
        parts.uri.query().unwrap_or_default().as_bytes(),
    ));
    query
        .append_pair("namespace", &scope.namespace)
        .append_pair("tenant", &scope.tenant);
    let uri = format!("/?{}", query.finish())
        .parse()
        .map_err(|_| rejection(StatusCode::BAD_REQUEST, "invalid query"))?;
    Query::<T>::try_from_uri(&uri)
        .map(|Query(value)| value)
        .map_err(IntoResponse::into_response)
}

/// Declare known root context positions on internal request models.
macro_rules! scoped_body {
    ($($ty:ty),+ $(,)?) => {$ (
        impl $crate::auth::context::ScopedBody for $ty {
            fn bind_context(body: &mut serde_json::Value, scope: &acteon_core::TenantContext)
                -> Result<(), axum::response::Response> {
                $crate::auth::context::bind_root(body, scope)
            }
        }
    )+};
}
pub(crate) use scoped_body;

/// Resource path parameters preceded internally by credential-derived context.
/// The URL contains resource identifiers only, never tenant or namespace slots.
pub struct ScopedPath<T>(pub T);

pub trait ScopedPathParams: Sized {
    fn bind_path(scope: TenantContext, values: Vec<String>) -> Result<Self, Response>;
}

impl<S: Send + Sync, T: ScopedPathParams> FromRequestParts<S> for ScopedPath<T> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        reject_scope_query(parts.uri.query())?;
        let scope = authenticated_scope(parts)?;
        let axum::extract::Path(params) =
            axum::extract::Path::<Vec<(String, String)>>::from_request_parts(parts, state)
                .await
                .map_err(IntoResponse::into_response)?;
        if params
            .iter()
            .any(|(key, _)| matches!(key.as_str(), "tenant" | "namespace"))
        {
            return Err(rejection(
                StatusCode::INTERNAL_SERVER_ERROR,
                "route still exposes credential context",
            ));
        }
        T::bind_path(scope, params.into_iter().map(|(_, value)| value).collect()).map(Self)
    }
}

macro_rules! scoped_path_params {
    ($($name:ident),* $(,)?) => {
        impl<$($name: std::str::FromStr),*> ScopedPathParams for (String, String, $($name,)*) {
            #[allow(non_snake_case, unused_mut)]
            fn bind_path(scope: TenantContext, values: Vec<String>) -> Result<Self, Response> {
                let mut values = values.into_iter();
                $(let $name = values.next().and_then(|v| v.parse::<$name>().ok())
                    .ok_or_else(|| rejection(StatusCode::BAD_REQUEST, "invalid resource path"))?;)*
                if values.next().is_some() {
                    return Err(rejection(StatusCode::BAD_REQUEST, "unexpected resource path"));
                }
                Ok((scope.namespace, scope.tenant, $($name,)*))
            }
        }
    };
}
scoped_path_params!();
scoped_path_params!(A);
scoped_path_params!(A, B);
scoped_path_params!(A, B, C);
scoped_path_params!(A, B, C, D);
scoped_path_params!(A, B, C, D, E);
