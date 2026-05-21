//! A2A Discovery + `AgentCard` CRUD (Phase 3.1).
//!
//! Two surfaces, sharing a single `AgentCard` storage row at
//! [`KeyKind::BusAgentCard`]:
//!
//! - **Authenticated CRUD** — `PUT` / `GET` / `DELETE` at
//!   `/v1/bus/agents/{namespace}/{tenant}/{agent_id}/card`. Operators
//!   register an A2A `AgentCard` against an already-registered agent;
//!   the agent's lean record at [`KeyKind::BusAgent`] gains
//!   `has_agent_card = true` so the hot heartbeat / list / route
//!   path can short-circuit without loading the verbose card body.
//! - **Public discovery** — `GET /a2a/{namespace}/{tenant}/.well-known/agent.json`.
//!   Per the A2A spec, agent discovery is public and unauthenticated.
//!   Acteon serves one A2A endpoint per `(namespace, tenant)`; the
//!   discovery response is the single card if the tenant has one
//!   agent registered, or an aggregated tenant card (combining
//!   skills, interfaces, and security schemes) if it has several.
//!
//! V1 scope: the routes above. Two follow-ups noted in the design
//! doc are out of scope here — the JSON-RPC `agent/getAuthenticatedExtendedCard`
//! method and Phase 4 card signing.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use acteon_core::{Agent, AgentCard, AgentCardValidationError, SecurityScheme};
use acteon_state::{CasResult, KeyKind, StateKey, StateStore};

use super::AppState;
use super::schemas::ErrorResponse;
use crate::auth::identity::CallerIdentity;
use crate::auth::role::Permission;

/// Synthetic provider name authorization uses for A2A card / discovery
/// operations — mirrors the `a2a` value used by the A2A protocol
/// endpoints (see [`crate::api::a2a`]).
const A2A_PROVIDER: &str = "a2a";

/// `agent_id` used for an aggregated tenant discovery card.
const TENANT_AGGREGATE_AGENT_ID: &str = "tenant";

/// `version` stamped on an aggregated discovery card. Bump if the
/// aggregation rule changes in a backwards-incompatible way.
const AGGREGATE_CARD_VERSION: &str = "aggregate.v1";

fn forbidden(msg: String) -> Response {
    (StatusCode::FORBIDDEN, Json(ErrorResponse { error: msg })).into_response()
}

fn bad_request(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg })).into_response()
}

fn internal(msg: String) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse { error: msg }),
    )
        .into_response()
}

fn not_found(msg: String) -> Response {
    (StatusCode::NOT_FOUND, Json(ErrorResponse { error: msg })).into_response()
}

/// Returns `Some(response)` to short-circuit when the caller is not
/// authorized to manage this tenant's `AgentCard`; `None` when allowed.
/// The `Option` shape (rather than `Result`) keeps the function below
/// clippy's `result_large_err` threshold — axum `Response` is a
/// sizeable enum.
fn authorize_card_op(identity: &CallerIdentity, namespace: &str, tenant: &str) -> Option<Response> {
    if !identity.role.has_permission(Permission::Dispatch) {
        return Some(forbidden(
            "card management requires the dispatch permission (admin or operator role)".into(),
        ));
    }
    if !identity.is_authorized(tenant, namespace, A2A_PROVIDER, "card") {
        return Some(forbidden(format!(
            "forbidden: no grant covers tenant={tenant}, namespace={namespace}, provider={A2A_PROVIDER}"
        )));
    }
    None
}

// ---------------------------------------------------------------------
// Storage helpers
// ---------------------------------------------------------------------

fn card_key(namespace: &str, tenant: &str, agent_id: &str) -> StateKey {
    StateKey::new(namespace, tenant, KeyKind::BusAgentCard, agent_id)
}

fn agent_key(namespace: &str, tenant: &str, agent_id: &str) -> StateKey {
    StateKey::new(namespace, tenant, KeyKind::BusAgent, agent_id)
}

async fn load_card(
    store: &Arc<dyn StateStore>,
    namespace: &str,
    tenant: &str,
    agent_id: &str,
) -> Result<Option<AgentCard>, String> {
    match store.get(&card_key(namespace, tenant, agent_id)).await {
        Ok(Some(raw)) => serde_json::from_str(&raw)
            .map(Some)
            .map_err(|e| format!("corrupt agent_card row: {e}")),
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Toggle `Agent.has_agent_card` for `agent_id` under CAS. The agent
/// row is the hot path that listings + the heartbeat reaper read; the
/// flag is what they key off to decide whether to fetch the full card.
async fn set_agent_card_flag(
    store: &Arc<dyn StateStore>,
    namespace: &str,
    tenant: &str,
    agent_id: &str,
    flag: bool,
) -> Result<bool, String> {
    let key = agent_key(namespace, tenant, agent_id);
    // A small CAS loop — concurrent updates to the same Agent row are
    // bounded by the bus's own agent CRUD, but a contended write
    // shouldn't lose the flag flip.
    for _ in 0..acteon_gateway::A2A_MAX_CAS_RETRY_ATTEMPTS {
        let Some((raw, version)) = store.get_versioned(&key).await.map_err(|e| e.to_string())?
        else {
            return Ok(false); // agent missing
        };
        let mut agent: Agent =
            serde_json::from_str(&raw).map_err(|e| format!("corrupt agent row: {e}"))?;
        if agent.has_agent_card == flag {
            return Ok(true);
        }
        agent.has_agent_card = flag;
        let payload = serde_json::to_string(&agent).map_err(|e| format!("agent serialize: {e}"))?;
        match store
            .compare_and_swap(&key, version, &payload, None)
            .await
            .map_err(|e| e.to_string())?
        {
            CasResult::Ok => return Ok(true),
            // Lost race; re-read and retry.
            CasResult::Conflict { .. } => {}
        }
    }
    Err("CAS contention exhausted while updating Agent.has_agent_card".into())
}

fn validation_message(e: &AgentCardValidationError) -> String {
    e.to_string()
}

// ---------------------------------------------------------------------
// Authenticated CRUD
// ---------------------------------------------------------------------

/// `Some(response)` if the card body's identity doesn't match the URL
/// path triple; `None` when they agree.
fn check_card_identity(
    namespace: &str,
    tenant: &str,
    agent_id: &str,
    card: &AgentCard,
) -> Option<Response> {
    if card.namespace != namespace || card.tenant != tenant || card.agent_id != agent_id {
        return Some(bad_request(format!(
            "card identity ({}/{}/{}) does not match path ({namespace}/{tenant}/{agent_id})",
            card.namespace, card.tenant, card.agent_id,
        )));
    }
    None
}

/// `PUT /v1/bus/agents/{namespace}/{tenant}/{agent_id}/card`
pub async fn put_agent_card(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, agent_id)): Path<(String, String, String)>,
    Json(mut card): Json<AgentCard>,
) -> Response {
    if let Some(resp) = authorize_card_op(&identity, &namespace, &tenant) {
        return resp;
    }
    if let Some(resp) = check_card_identity(&namespace, &tenant, &agent_id, &card) {
        return resp;
    }
    card.updated_at = chrono::Utc::now();
    if let Err(e) = card.validate() {
        return bad_request(validation_message(&e));
    }
    let store: Arc<dyn StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    let raw = match serde_json::to_string(&card) {
        Ok(r) => r,
        Err(e) => return internal(format!("serialize card: {e}")),
    };
    if let Err(e) = store
        .set(&card_key(&namespace, &tenant, &agent_id), &raw, None)
        .await
    {
        return internal(format!("write card: {e}"));
    }
    match set_agent_card_flag(&store, &namespace, &tenant, &agent_id, true).await {
        Ok(true) => {}
        Ok(false) => {
            return bad_request(format!(
                "agent {namespace}/{tenant}/{agent_id} is not registered; register it before attaching a card",
            ));
        }
        Err(e) => return internal(format!("set has_agent_card flag: {e}")),
    }
    (StatusCode::OK, Json(card)).into_response()
}

/// `GET /v1/bus/agents/{namespace}/{tenant}/{agent_id}/card`
pub async fn get_agent_card(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, agent_id)): Path<(String, String, String)>,
) -> Response {
    if let Some(resp) = authorize_card_op(&identity, &namespace, &tenant) {
        return resp;
    }
    let store: Arc<dyn StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    match load_card(&store, &namespace, &tenant, &agent_id).await {
        Ok(Some(card)) => (StatusCode::OK, Json(card)).into_response(),
        Ok(None) => not_found(format!("no agent_card for {namespace}/{tenant}/{agent_id}")),
        Err(e) => internal(e),
    }
}

/// `DELETE /v1/bus/agents/{namespace}/{tenant}/{agent_id}/card`
pub async fn delete_agent_card(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((namespace, tenant, agent_id)): Path<(String, String, String)>,
) -> Response {
    if let Some(resp) = authorize_card_op(&identity, &namespace, &tenant) {
        return resp;
    }
    let store: Arc<dyn StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    if let Err(e) = store
        .delete(&card_key(&namespace, &tenant, &agent_id))
        .await
    {
        return internal(format!("delete card: {e}"));
    }
    // Best-effort flag flip — if the agent is gone (already deleted),
    // there's nothing to update.
    let _ = set_agent_card_flag(&store, &namespace, &tenant, &agent_id, false).await;
    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------
// Public discovery
// ---------------------------------------------------------------------

/// `GET /a2a/{namespace}/{tenant}/.well-known/agent.json`
///
/// Public, unauthenticated, per the A2A spec. Returns the tenant's
/// `AgentCard`. If one card is registered, the card is returned
/// verbatim. If several are registered, an aggregated tenant card is
/// returned — skills, interfaces, and security schemes are merged so a
/// client sees the union of capabilities exposed at the
/// `(namespace, tenant)` endpoint.
pub async fn discover_agent(
    State(state): State<AppState>,
    Path((namespace, tenant)): Path<(String, String)>,
) -> Response {
    match resolve_tenant_card(&state, &namespace, &tenant).await {
        Ok(Some(card)) => (StatusCode::OK, Json(card)).into_response(),
        Ok(None) => not_found(format!(
            "no agent registered with a published card under {namespace}/{tenant}",
        )),
        Err(e) => internal(format!("scan agent_cards: {e}")),
    }
}

/// Load every `AgentCard` registered under (`namespace`, `tenant`) and
/// reduce them to a single discovery card — verbatim when exactly one
/// card is published, otherwise via [`aggregate_tenant_card`].
///
/// Returns `Ok(None)` when no card is published; the caller decides
/// whether that is a 404 (REST) or a JSON-RPC `MethodNotFound`.
/// Returns `Err(String)` only when the underlying state-store scan
/// itself fails.
pub(crate) async fn resolve_tenant_card(
    state: &AppState,
    namespace: &str,
    tenant: &str,
) -> Result<Option<AgentCard>, String> {
    let store: Arc<dyn StateStore> = {
        let gw = state.gateway.read().await;
        gw.state_store().clone()
    };
    let entries = store
        .scan_keys(namespace, tenant, KeyKind::BusAgentCard, None)
        .await
        .map_err(|e| e.to_string())?;
    let mut cards: Vec<AgentCard> = Vec::with_capacity(entries.len());
    for (_, raw) in entries {
        if let Ok(card) = serde_json::from_str::<AgentCard>(&raw) {
            cards.push(card);
        }
    }
    let mut card = match cards.len() {
        0 => return Ok(None),
        1 => cards.into_iter().next().expect("len() == 1"),
        _ => aggregate_tenant_card(namespace, tenant, cards),
    };
    // Layer in the gateway's intrinsic security schemes so a client
    // reading this card knows how to authenticate to Acteon itself.
    // User-published aliases win on collision (the entry-API path is
    // `or_insert`, not `insert`), so a tenant that explicitly
    // documents its own scheme under `acteon.bearer` is not
    // overwritten.
    enrich_with_intrinsic_schemes(&mut card, state.auth.is_some());
    Ok(Some(card))
}

/// Reserved alias of the gateway's intrinsic `Authorization: Bearer`
/// scheme. The middleware accepts both JWT and raw API-key tokens on
/// this header, so the wire shape is plain Bearer regardless.
const INTRINSIC_BEARER_ALIAS: &str = "acteon.bearer";

/// Reserved alias of the gateway's intrinsic `X-API-Key` header
/// scheme — the second auth path the middleware recognizes.
const INTRINSIC_API_KEY_ALIAS: &str = "acteon.apiKey";

/// Build the set of intrinsic security schemes the gateway itself
/// implements. Returns an empty list when auth is disabled — the
/// discovery card honestly reflects "no scheme required" in that
/// case rather than advertising a Bearer the server will accept any
/// token for.
///
/// Split out from [`enrich_with_intrinsic_schemes`] so the scheme set
/// is unit-testable without an `AppState`.
fn intrinsic_security_schemes(auth_enabled: bool) -> Vec<(&'static str, SecurityScheme)> {
    if !auth_enabled {
        return Vec::new();
    }
    vec![
        (
            INTRINSIC_BEARER_ALIAS,
            SecurityScheme::HttpAuth {
                scheme_name: "bearer".to_string(),
                bearer_format: None,
            },
        ),
        (
            INTRINSIC_API_KEY_ALIAS,
            SecurityScheme::ApiKey {
                name: "X-API-Key".to_string(),
                location: "header".to_string(),
            },
        ),
    ]
}

/// Layer the gateway's intrinsic security schemes into the card
/// returned by `resolve_tenant_card`. Uses `entry().or_insert(…)`
/// semantics so a user-published alias under the reserved
/// `acteon.bearer` / `acteon.apiKey` keys is preserved verbatim
/// (the user is unambiguously documenting their own scheme).
fn enrich_with_intrinsic_schemes(card: &mut AgentCard, auth_enabled: bool) {
    for (alias, scheme) in intrinsic_security_schemes(auth_enabled) {
        card.security_schemes
            .entry(alias.to_string())
            .or_insert(scheme);
    }
}

/// Combine the skills, interfaces, and security schemes of several
/// agent cards into a single tenant-level discovery card. Identity
/// fields collapse onto the synthetic `tenant` agent id; the per-agent
/// `provider` is dropped (no canonical choice across agents); the
/// name is a synthetic `"{tenant} A2A endpoint"`. Skill ids and
/// security-scheme aliases get the source `agent_id` appended on
/// collision so the union stays addressable.
fn aggregate_tenant_card(namespace: &str, tenant: &str, cards: Vec<AgentCard>) -> AgentCard {
    let now = chrono::Utc::now();
    let mut agg = AgentCard::new(
        TENANT_AGGREGATE_AGENT_ID,
        namespace,
        tenant,
        format!("{tenant} A2A endpoint"),
        AGGREGATE_CARD_VERSION,
    );
    agg.created_at = now;
    agg.updated_at = now;
    agg.description = Some(format!(
        "Aggregated A2A discovery card for the {tenant} tenant (combining {} registered agents).",
        cards.len(),
    ));

    let mut skill_names: HashSet<String> = HashSet::new();
    let mut iface_keys: HashSet<(String, String)> = HashSet::new();
    let mut scheme_aliases: HashSet<String> = HashSet::new();

    for card in cards {
        let owner = card.agent_id.clone();
        for mut skill in card.skills {
            if skill_names.contains(&skill.name) {
                skill.name = format!("{}@{owner}", skill.name);
            }
            skill_names.insert(skill.name.clone());
            agg.skills.push(skill);
        }
        for iface in card.interfaces {
            let k = (iface.kind.clone(), iface.url.clone());
            if iface_keys.contains(&k) {
                continue;
            }
            iface_keys.insert(k);
            agg.interfaces.push(iface);
        }
        for (alias, scheme) in card.security_schemes {
            let final_alias = if scheme_aliases.contains(&alias) {
                format!("{alias}@{owner}")
            } else {
                alias
            };
            scheme_aliases.insert(final_alias.clone());
            agg.security_schemes.insert(final_alias, scheme);
        }
        // Capabilities: OR-merge (any agent's enabled capability lifts
        // the aggregate).
        agg.capabilities.streaming |= card.capabilities.streaming;
        agg.capabilities.push_notifications |= card.capabilities.push_notifications;
        agg.capabilities.extended_agent_card |= card.capabilities.extended_agent_card;
        // Extensions: include each at most once by uri.
        for ext in card.extensions {
            if !agg.extensions.iter().any(|e| e.uri == ext.uri) {
                agg.extensions.push(ext);
            }
        }
    }

    agg
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::AgentCapabilities;
    use acteon_state_memory::MemoryStateStore;

    fn store() -> Arc<dyn StateStore> {
        Arc::new(MemoryStateStore::new())
    }

    fn sample_card(agent_id: &str) -> AgentCard {
        let mut c = AgentCard::new(agent_id, "agents", "demo", agent_id, "1.0");
        c.capabilities = AgentCapabilities {
            streaming: true,
            ..Default::default()
        };
        c
    }

    async fn write_card(store: &Arc<dyn StateStore>, card: &AgentCard) {
        let key = card_key(&card.namespace, &card.tenant, &card.agent_id);
        let raw = serde_json::to_string(card).unwrap();
        store.set(&key, &raw, None).await.unwrap();
    }

    #[tokio::test]
    async fn aggregate_card_unions_skills_and_capabilities() {
        let mut a = sample_card("a-1");
        a.skills.push(acteon_core::Skill::new("echo"));
        let mut b = sample_card("a-2");
        b.capabilities.push_notifications = true;
        b.skills.push(acteon_core::Skill::new("echo")); // colliding name
        b.skills.push(acteon_core::Skill::new("summarize"));

        let agg = aggregate_tenant_card("agents", "demo", vec![a, b]);

        assert_eq!(agg.agent_id, TENANT_AGGREGATE_AGENT_ID);
        // Capabilities OR'd across cards.
        assert!(agg.capabilities.streaming);
        assert!(agg.capabilities.push_notifications);
        // Skill names deduped: the collision was suffixed with @agent_id.
        let names: Vec<&str> = agg.skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"echo@a-2"));
        assert!(names.contains(&"summarize"));
    }

    #[tokio::test]
    async fn aggregate_card_collapses_duplicate_interfaces() {
        let mut a = sample_card("a-1");
        a.interfaces.push(acteon_core::AgentCardInterface {
            kind: "json-rpc".into(),
            url: "https://example.test/a2a/agents/demo".into(),
        });
        let mut b = sample_card("a-2");
        b.interfaces.push(acteon_core::AgentCardInterface {
            kind: "json-rpc".into(),
            url: "https://example.test/a2a/agents/demo".into(),
        });

        let agg = aggregate_tenant_card("agents", "demo", vec![a, b]);
        assert_eq!(agg.interfaces.len(), 1);
    }

    #[tokio::test]
    async fn load_card_round_trips_through_state_store() {
        let s = store();
        let card = sample_card("a-1");
        write_card(&s, &card).await;
        let got = load_card(&s, "agents", "demo", "a-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.agent_id, card.agent_id);
        assert_eq!(got.capabilities.streaming, card.capabilities.streaming);
        assert!(
            load_card(&s, "agents", "demo", "missing")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn aggregate_includes_each_extension_at_most_once() {
        let mut a = sample_card("a-1");
        a.extensions.push(acteon_core::AgentCardExtension {
            uri: "x://shared".into(),
            version: "1".into(),
            required: false,
        });
        let mut b = sample_card("a-2");
        b.extensions.push(acteon_core::AgentCardExtension {
            uri: "x://shared".into(),
            version: "1".into(),
            required: true,
        });
        b.extensions.push(acteon_core::AgentCardExtension {
            uri: "x://b-only".into(),
            version: "1".into(),
            required: false,
        });
        let agg = aggregate_tenant_card("agents", "demo", vec![a, b]);
        let uris: Vec<&str> = agg.extensions.iter().map(|e| e.uri.as_str()).collect();
        assert_eq!(uris.len(), 2);
        assert!(uris.contains(&"x://shared"));
        assert!(uris.contains(&"x://b-only"));
    }

    // --- Intrinsic security schemes (Phase 4.3) ---

    #[test]
    fn intrinsic_schemes_empty_when_auth_disabled() {
        // An auth-disabled server has no scheme to advertise — the
        // discovery card must not lie about a Bearer it would accept
        // any token for.
        assert!(intrinsic_security_schemes(false).is_empty());
    }

    #[test]
    fn intrinsic_schemes_lists_bearer_and_api_key_when_auth_enabled() {
        let schemes = intrinsic_security_schemes(true);
        let aliases: Vec<&str> = schemes.iter().map(|(a, _)| *a).collect();
        assert_eq!(
            aliases,
            vec![INTRINSIC_BEARER_ALIAS, INTRINSIC_API_KEY_ALIAS]
        );
        // Sanity on the scheme shape — Bearer carries no format hint.
        match &schemes[0].1 {
            SecurityScheme::HttpAuth {
                scheme_name,
                bearer_format,
            } => {
                assert_eq!(scheme_name, "bearer");
                assert!(bearer_format.is_none());
            }
            other => panic!("expected HttpAuth Bearer, got {other:?}"),
        }
        // API-key scheme reads from the `X-API-Key` header — the
        // exact header the middleware looks for.
        match &schemes[1].1 {
            SecurityScheme::ApiKey { name, location } => {
                assert_eq!(name, "X-API-Key");
                assert_eq!(location, "header");
            }
            other => panic!("expected ApiKey scheme, got {other:?}"),
        }
    }

    #[test]
    fn enrich_with_intrinsic_schemes_is_a_noop_when_auth_disabled() {
        let mut card = sample_card("a-1");
        let before = card.security_schemes.len();
        enrich_with_intrinsic_schemes(&mut card, false);
        assert_eq!(card.security_schemes.len(), before);
    }

    #[test]
    fn enrich_with_intrinsic_schemes_adds_under_reserved_aliases() {
        let mut card = sample_card("a-1");
        enrich_with_intrinsic_schemes(&mut card, true);
        assert!(card.security_schemes.contains_key(INTRINSIC_BEARER_ALIAS));
        assert!(card.security_schemes.contains_key(INTRINSIC_API_KEY_ALIAS));
    }

    #[test]
    fn enrich_with_intrinsic_schemes_preserves_user_published_aliases() {
        let mut card = sample_card("a-1");
        // User explicitly publishes their own bearer under the reserved
        // alias. The enrichment must not clobber it.
        let user_scheme = SecurityScheme::HttpAuth {
            scheme_name: "bearer".into(),
            bearer_format: Some("user-JWT-format".into()),
        };
        card.security_schemes
            .insert(INTRINSIC_BEARER_ALIAS.into(), user_scheme);
        enrich_with_intrinsic_schemes(&mut card, true);
        match card.security_schemes.get(INTRINSIC_BEARER_ALIAS) {
            Some(SecurityScheme::HttpAuth { bearer_format, .. }) => {
                assert_eq!(
                    bearer_format.as_deref(),
                    Some("user-JWT-format"),
                    "user-published scheme must not be clobbered by intrinsic enrichment"
                );
            }
            other => panic!("expected user scheme to survive, got {other:?}"),
        }
        // The other intrinsic alias (apiKey) was absent, so it WAS added.
        assert!(card.security_schemes.contains_key(INTRINSIC_API_KEY_ALIAS));
    }
}
