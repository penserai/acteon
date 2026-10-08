//! Prevent unjournaled registry metadata writes in any persisted execution scope.
use super::AppState;
use acteon_governance::{AuthorityCoordinator, COORDINATOR_KIND, ScopePurpose};
use acteon_state::{KeyKind, StateKey};
use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

pub(super) async fn refuse_unjournaled_write(
    state: &AppState,
    namespace: &str,
    tenant: &str,
) -> Option<Response> {
    let store = state.gateway.read().await.state_store().clone();
    let key = StateKey::new(
        namespace,
        tenant,
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    let result = match store.get(&key).await {
        Ok(None) => return None,
        Ok(Some(_)) => AuthorityCoordinator::connect(store, namespace, tenant).await,
        Err(_) => return Some(unavailable()),
    };
    match result {
        Ok(coordinator) => match coordinator.snapshot().await {
            Ok(snapshot) if matches!(snapshot.purpose, ScopePurpose::Execution) => Some((
                StatusCode::CONFLICT, [(header::CACHE_CONTROL, "no-store")],
                Json(serde_json::json!({"error":"governed_registry_mutation_required", "code":"governed_registry_mutation_required"})),
            ).into_response()),
            Ok(_) => None,
            Err(_) => Some(unavailable()),
        },
        Err(_) => Some(unavailable()),
    }
}
fn unavailable() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({"error":"registry_authority_unavailable", "code":"registry_authority_unavailable"}))).into_response()
}
