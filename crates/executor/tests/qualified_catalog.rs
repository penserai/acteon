use std::sync::Arc;

use acteon_core::{Action, ResourceKind, ResourceRef};
use acteon_executor::catalog::{CatalogError, QualifiedProviderCatalog};
use acteon_executor::governed::BoundProvider;
use acteon_provider::{DynProvider, LogProvider};

fn provider(name: &str) -> Arc<dyn DynProvider> {
    Arc::new(LogProvider::new(name))
}
fn binding(
    provider: Arc<dyn DynProvider>,
    revision: &str,
    extra: Vec<ResourceRef>,
) -> BoundProvider {
    BoundProvider::new_trusted(
        provider,
        &ResourceRef::new(ResourceKind::Endpoint, "city", "tenant", "endpoint-v1").unwrap(),
        "work",
        revision,
        extra,
    )
    .unwrap()
}
fn action() -> Action {
    Action::new("city", "tenant", "original", "work", serde_json::json!({}))
}

#[test]
fn selected_instance_and_primary_action_cannot_borrow_qualification() {
    let selected = provider("fallback");
    let same_name = provider("fallback");
    let internal = ResourceRef::new(ResourceKind::Action, "city", "tenant", "internal").unwrap();
    let catalog = QualifiedProviderCatalog::new_trusted(vec![binding(
        selected.clone(),
        "v1",
        vec![internal.clone()],
    )])
    .unwrap();
    let mut request = action();
    request
        .metadata
        .labels
        .insert("provider".into(), "fallback".into());
    assert!(catalog.resolve(&request, &selected).is_ok());
    assert!(matches!(
        catalog.resolve(&request, &same_name),
        Err(CatalogError::Unqualified)
    ));
    assert!(catalog.resolve(&request, &provider("original")).is_err());
    let definitions = catalog.definitions("city", "tenant");
    assert_eq!(definitions[0].provider, "fallback");
    assert!(definitions[0].effect.resources.contains(&internal));
    request.action_type = "internal".into();
    assert!(catalog.resolve(&request, &selected).is_err());
    request.action_type = "work".into();
    request.tenant = "other".into();
    assert!(catalog.resolve(&request, &selected).is_err());
    assert_eq!(catalog.definitions("city", "other"), Vec::new());
}

#[test]
fn descriptor_fingerprint_is_canonical_but_route_conflicts_are_rejected() {
    let a = provider("a");
    let b = provider("b");
    let first = binding(a.clone(), "v1", vec![]);
    let second = binding(b.clone(), "v1", vec![]);
    let forward =
        QualifiedProviderCatalog::new_trusted(vec![first.clone(), second.clone()]).unwrap();
    let reverse = QualifiedProviderCatalog::new_trusted(vec![second, first.clone()]).unwrap();
    assert_eq!(forward.fingerprint(), reverse.fingerprint());
    assert!(matches!(
        QualifiedProviderCatalog::new_trusted(vec![first.clone(), first]),
        Err(CatalogError::Ambiguous)
    ));
    let revised = QualifiedProviderCatalog::new_trusted(vec![
        binding(a, "v2", vec![]),
        binding(b, "v1", vec![]),
    ])
    .unwrap();
    assert_ne!(forward.fingerprint(), revised.fingerprint());
    let replicated = QualifiedProviderCatalog::new_trusted(vec![
        binding(provider("a"), "v1", vec![]),
        binding(provider("b"), "v1", vec![]),
    ])
    .unwrap();
    assert_eq!(forward.fingerprint(), replicated.fingerprint());
    let mut descriptions = forward.definitions("city", "tenant");
    descriptions[0].revision = "forged".into();
    assert_eq!(forward.fingerprint(), reverse.fingerprint());
    assert_ne!(forward.definitions("city", "tenant")[0].revision, "forged");
}

#[test]
fn metadata_does_not_contain_transport_inputs() {
    let selected = provider("fallback");
    let catalog = QualifiedProviderCatalog::new_trusted(vec![binding(
        selected.clone(),
        "opaque-version",
        vec![],
    )])
    .unwrap();
    let mut request = action();
    request.payload = serde_json::json!({"secret": "request-secret"});
    let bytes = serde_json::to_string(&catalog.definitions("city", "tenant")).unwrap();
    assert!(!bytes.contains("request-secret"));
    assert!(catalog.clone().resolve(&request, &selected).is_ok());
}

#[test]
fn history_catalog_is_explicit_and_cannot_qualify_any_actual_provider() {
    assert!(matches!(
        QualifiedProviderCatalog::new_trusted(Vec::new()),
        Err(CatalogError::Capacity)
    ));
    let catalog = QualifiedProviderCatalog::for_history();
    assert_eq!(catalog.definitions("city", "tenant"), Vec::new());
    assert!(matches!(
        catalog.resolve(&action(), &provider("original")),
        Err(CatalogError::Unqualified)
    ));
    assert_eq!(
        catalog.fingerprint(),
        QualifiedProviderCatalog::for_history().fingerprint()
    );
}
