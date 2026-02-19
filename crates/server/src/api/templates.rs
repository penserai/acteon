//! Template and profile API endpoints.
//!
//! CRUD operations for payload templates, template profiles, and preview rendering.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use acteon_core::template::{
    Template, TemplateProfile, TemplateProfileField, validate_template_content,
    validate_template_name,
};
use acteon_state::{KeyKind, StateKey};

use super::AppState;
use super::schemas::ErrorResponse;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// Request body for creating a template.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTemplateRequest {
    /// Template name (unique within namespace + tenant).
    #[schema(example = "welcome-email")]
    pub name: String,
    /// Namespace this template belongs to.
    #[schema(example = "notifications")]
    pub namespace: String,
    /// Tenant this template belongs to.
    #[schema(example = "tenant-1")]
    pub tenant: String,
    /// Raw `MiniJinja` template content.
    pub content: String,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Arbitrary key-value labels.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Request body for updating a template.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateTemplateRequest {
    /// Updated template content.
    #[serde(default)]
    pub content: Option<String>,
    /// Updated description.
    #[serde(default)]
    pub description: Option<String>,
    /// Updated labels.
    #[serde(default)]
    pub labels: Option<HashMap<String, String>>,
}

/// Template response.
#[derive(Debug, Serialize, ToSchema)]
pub struct TemplateResponse {
    /// Unique template ID.
    pub id: String,
    /// Template name.
    pub name: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Raw template content.
    pub content: String,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// When the template was created.
    pub created_at: DateTime<Utc>,
    /// When the template was last updated.
    pub updated_at: DateTime<Utc>,
    /// Arbitrary labels.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

/// Response for listing templates.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListTemplatesResponse {
    /// List of templates.
    pub templates: Vec<TemplateResponse>,
    /// Total count of results returned.
    pub count: usize,
}

/// Request body for creating a template profile.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateProfileRequest {
    /// Profile name (unique within namespace + tenant).
    #[schema(example = "welcome-profile")]
    pub name: String,
    /// Namespace this profile belongs to.
    #[schema(example = "notifications")]
    pub namespace: String,
    /// Tenant this profile belongs to.
    #[schema(example = "tenant-1")]
    pub tenant: String,
    /// Field-to-template mappings.
    pub fields: HashMap<String, TemplateProfileField>,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Arbitrary key-value labels.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Request body for updating a template profile.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateProfileRequest {
    /// Updated field mappings.
    #[serde(default)]
    pub fields: Option<HashMap<String, TemplateProfileField>>,
    /// Updated description.
    #[serde(default)]
    pub description: Option<String>,
    /// Updated labels.
    #[serde(default)]
    pub labels: Option<HashMap<String, String>>,
}

/// Template profile response.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileResponse {
    /// Unique profile ID.
    pub id: String,
    /// Profile name.
    pub name: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Field-to-template mappings.
    pub fields: HashMap<String, TemplateProfileField>,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// When the profile was created.
    pub created_at: DateTime<Utc>,
    /// When the profile was last updated.
    pub updated_at: DateTime<Utc>,
    /// Arbitrary labels.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

/// Response for listing template profiles.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListProfilesResponse {
    /// List of profiles.
    pub profiles: Vec<ProfileResponse>,
    /// Total count of results returned.
    pub count: usize,
}

/// Request body for template render preview.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RenderPreviewRequest {
    /// Profile name to render.
    pub profile: String,
    /// Namespace of the profile.
    pub namespace: String,
    /// Tenant of the profile.
    pub tenant: String,
    /// Test payload variables.
    pub payload: serde_json::Value,
}

/// Template render preview response.
#[derive(Debug, Serialize, ToSchema)]
pub struct RenderPreviewResponse {
    /// Rendered field values.
    pub rendered: HashMap<String, String>,
}

/// Query parameters for listing templates and profiles.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListTemplatesParams {
    /// Filter by namespace (optional).
    #[serde(default)]
    pub namespace: Option<String>,
    /// Filter by tenant (optional).
    #[serde(default)]
    pub tenant: Option<String>,
    /// Maximum number of results (default: 100).
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Number of results to skip (default: 0).
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    100
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const TEMPLATE_STORE_NS: &str = "_system";
const TEMPLATE_STORE_TENANT: &str = "_templates";

fn template_state_key(id: &str) -> StateKey {
    StateKey::new(
        TEMPLATE_STORE_NS,
        TEMPLATE_STORE_TENANT,
        KeyKind::Template,
        id,
    )
}

fn template_index_key(namespace: &str, tenant: &str, name: &str) -> StateKey {
    let suffix = format!("idx:{namespace}:{tenant}:{name}");
    StateKey::new(
        TEMPLATE_STORE_NS,
        TEMPLATE_STORE_TENANT,
        KeyKind::Template,
        &suffix,
    )
}

fn profile_state_key(id: &str) -> StateKey {
    StateKey::new(
        TEMPLATE_STORE_NS,
        TEMPLATE_STORE_TENANT,
        KeyKind::TemplateProfile,
        id,
    )
}

fn profile_index_key(namespace: &str, tenant: &str, name: &str) -> StateKey {
    let suffix = format!("idx:{namespace}:{tenant}:{name}");
    StateKey::new(
        TEMPLATE_STORE_NS,
        TEMPLATE_STORE_TENANT,
        KeyKind::TemplateProfile,
        &suffix,
    )
}

async fn load_template(
    state_store: &dyn acteon_state::StateStore,
    id: &str,
) -> Result<Option<Template>, String> {
    let key = template_state_key(id);
    let value = state_store.get(&key).await.map_err(|e| e.to_string())?;
    match value {
        Some(data) => {
            let tpl = serde_json::from_str::<Template>(&data).map_err(|e| e.to_string())?;
            Ok(Some(tpl))
        }
        None => Ok(None),
    }
}

async fn load_profile(
    state_store: &dyn acteon_state::StateStore,
    id: &str,
) -> Result<Option<TemplateProfile>, String> {
    let key = profile_state_key(id);
    let value = state_store.get(&key).await.map_err(|e| e.to_string())?;
    match value {
        Some(data) => {
            let prof = serde_json::from_str::<TemplateProfile>(&data).map_err(|e| e.to_string())?;
            Ok(Some(prof))
        }
        None => Ok(None),
    }
}

fn template_to_response(t: &Template) -> TemplateResponse {
    TemplateResponse {
        id: t.id.clone(),
        name: t.name.clone(),
        namespace: t.namespace.clone(),
        tenant: t.tenant.clone(),
        content: t.content.clone(),
        description: t.description.clone(),
        created_at: t.created_at,
        updated_at: t.updated_at,
        labels: t.labels.clone(),
    }
}

fn profile_to_response(p: &TemplateProfile) -> ProfileResponse {
    ProfileResponse {
        id: p.id.clone(),
        name: p.name.clone(),
        namespace: p.namespace.clone(),
        tenant: p.tenant.clone(),
        fields: p.fields.clone(),
        description: p.description.clone(),
        created_at: p.created_at,
        updated_at: p.updated_at,
        labels: p.labels.clone(),
    }
}

fn error_response(status: StatusCode, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!(ErrorResponse {
            error: message.to_owned(),
        })),
    )
        .into_response()
}

/// Validate template content compiles as valid `MiniJinja`.
fn validate_template_syntax(content: &str) -> Result<(), String> {
    let env = minijinja::Environment::new();
    env.render_str(content, ()).map(|_| ()).or_else(|e| {
        let msg = e.to_string();
        // Missing variables are fine -- only syntax errors matter.
        if msg.contains("undefined") || msg.contains("unknown") {
            Ok(())
        } else {
            Err(format!("template syntax error: {e}"))
        }
    })
}

// ---------------------------------------------------------------------------
// Template handlers
// ---------------------------------------------------------------------------

/// `POST /v1/templates` -- create a template.
#[utoipa::path(
    post,
    path = "/v1/templates",
    tag = "Templates",
    summary = "Create a template",
    description = "Creates a new payload template. Validates name, content size, and `MiniJinja` syntax.",
    request_body(content = CreateTemplateRequest, description = "Template definition"),
    responses(
        (status = 201, description = "Template created", body = TemplateResponse),
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 409, description = "A template with this name already exists in the scope", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn create_template(
    State(state): State<AppState>,
    Json(req): Json<CreateTemplateRequest>,
) -> impl IntoResponse {
    if let Err(e) = validate_template_name(&req.name) {
        return error_response(StatusCode::BAD_REQUEST, &e);
    }
    if let Err(e) = validate_template_content(&req.content) {
        return error_response(StatusCode::BAD_REQUEST, &e);
    }
    if let Err(e) = validate_template_syntax(&req.content) {
        return error_response(StatusCode::BAD_REQUEST, &e);
    }

    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    // Check for duplicate.
    let idx_key = template_index_key(&req.namespace, &req.tenant, &req.name);
    match state_store.get(&idx_key).await {
        Ok(Some(_)) => {
            return error_response(
                StatusCode::CONFLICT,
                &format!(
                    "a template named '{}' already exists for namespace={} tenant={}",
                    req.name, req.namespace, req.tenant
                ),
            );
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        Ok(None) => {}
    }

    let now = Utc::now();
    let id = uuid::Uuid::new_v4().to_string();

    let template = Template {
        id: id.clone(),
        name: req.name,
        namespace: req.namespace,
        tenant: req.tenant,
        content: req.content,
        description: req.description,
        created_at: now,
        updated_at: now,
        labels: req.labels,
    };

    let key = template_state_key(&id);
    let data = match serde_json::to_string(&template) {
        Ok(d) => d,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("serialization error: {e}"),
            );
        }
    };
    if let Err(e) = state_store.set(&key, &data, None).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if let Err(e) = state_store.set(&idx_key, &id, None).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    drop(gw);

    let gw = state.gateway.read().await;
    gw.set_template(template.clone());

    let resp = template_to_response(&template);
    (StatusCode::CREATED, Json(serde_json::json!(resp))).into_response()
}

/// `GET /v1/templates` -- list templates.
#[utoipa::path(
    get,
    path = "/v1/templates",
    tag = "Templates",
    summary = "List templates",
    description = "Returns templates, optionally filtered by namespace and tenant.",
    params(ListTemplatesParams),
    responses(
        (status = 200, description = "Template list", body = ListTemplatesResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn list_templates(
    State(state): State<AppState>,
    Query(params): Query<ListTemplatesParams>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let results = match state_store.scan_keys_by_kind(KeyKind::Template).await {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let mut templates: Vec<TemplateResponse> = Vec::new();
    let mut skipped = 0usize;

    for (_key, value) in results {
        // Skip index keys (they store just the ID, not a full Template).
        let Ok(tpl) = serde_json::from_str::<Template>(&value) else {
            continue;
        };

        if let Some(ref ns) = params.namespace
            && tpl.namespace != *ns
        {
            continue;
        }
        if let Some(ref t) = params.tenant
            && tpl.tenant != *t
        {
            continue;
        }

        if skipped < params.offset {
            skipped += 1;
            continue;
        }
        if templates.len() >= params.limit {
            break;
        }

        templates.push(template_to_response(&tpl));
    }

    let count = templates.len();
    (
        StatusCode::OK,
        Json(serde_json::json!(ListTemplatesResponse {
            templates,
            count
        })),
    )
        .into_response()
}

/// `GET /v1/templates/{id}` -- get a single template.
#[utoipa::path(
    get,
    path = "/v1/templates/{id}",
    tag = "Templates",
    summary = "Get template details",
    description = "Returns the full details of a template.",
    params(("id" = String, Path, description = "Template ID")),
    responses(
        (status = 200, description = "Template details", body = TemplateResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn get_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    match load_template(state_store.as_ref(), &id).await {
        Ok(Some(tpl)) => (
            StatusCode::OK,
            Json(serde_json::json!(template_to_response(&tpl))),
        )
            .into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, &format!("template not found: {id}")),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

/// `PUT /v1/templates/{id}` -- update a template.
#[utoipa::path(
    put,
    path = "/v1/templates/{id}",
    tag = "Templates",
    summary = "Update a template",
    description = "Updates fields of an existing template.",
    params(("id" = String, Path, description = "Template ID")),
    request_body(content = UpdateTemplateRequest, description = "Fields to update"),
    responses(
        (status = 200, description = "Updated template", body = TemplateResponse),
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn update_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateTemplateRequest>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let mut tpl = match load_template(state_store.as_ref(), &id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, &format!("template not found: {id}"));
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    if let Some(ref content) = req.content {
        if let Err(e) = validate_template_content(content) {
            return error_response(StatusCode::BAD_REQUEST, &e);
        }
        if let Err(e) = validate_template_syntax(content) {
            return error_response(StatusCode::BAD_REQUEST, &e);
        }
        tpl.content.clone_from(content);
    }
    if let Some(desc) = req.description {
        tpl.description = Some(desc);
    }
    if let Some(labels) = req.labels {
        tpl.labels = labels;
    }
    tpl.updated_at = Utc::now();

    let key = template_state_key(&id);
    let data = match serde_json::to_string(&tpl) {
        Ok(d) => d,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("serialization error: {e}"),
            );
        }
    };
    if let Err(e) = state_store.set(&key, &data, None).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    drop(gw);

    let gw = state.gateway.read().await;
    gw.set_template(tpl.clone());

    let resp = template_to_response(&tpl);
    (StatusCode::OK, Json(serde_json::json!(resp))).into_response()
}

/// `DELETE /v1/templates/{id}` -- delete a template.
#[utoipa::path(
    delete,
    path = "/v1/templates/{id}",
    tag = "Templates",
    summary = "Delete a template",
    description = "Removes a template. Returns 409 if profiles reference it.",
    params(("id" = String, Path, description = "Template ID")),
    responses(
        (status = 204, description = "Template deleted"),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 409, description = "Template is referenced by profiles", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn delete_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let tpl = match load_template(state_store.as_ref(), &id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, &format!("template not found: {id}"));
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    // Check if any profiles reference this template.
    let referencing: Vec<String> = gw
        .template_profiles()
        .values()
        .filter(|p| p.namespace == tpl.namespace && p.tenant == tpl.tenant)
        .filter(|p| {
            p.fields.values().any(|f| {
                matches!(f, TemplateProfileField::Ref { template_ref } if template_ref == &tpl.name)
            })
        })
        .map(|p| p.name.clone())
        .collect();

    if !referencing.is_empty() {
        return error_response(
            StatusCode::CONFLICT,
            &format!(
                "template '{}' is referenced by profiles: {}",
                tpl.name,
                referencing.join(", ")
            ),
        );
    }

    let key = template_state_key(&id);
    if let Err(e) = state_store.delete(&key).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let idx_key = template_index_key(&tpl.namespace, &tpl.tenant, &tpl.name);
    let _ = state_store.delete(&idx_key).await;
    drop(gw);

    let gw = state.gateway.read().await;
    gw.remove_template(&tpl.namespace, &tpl.tenant, &tpl.name);

    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// Profile handlers
// ---------------------------------------------------------------------------

/// `POST /v1/templates/profiles` -- create a template profile.
#[utoipa::path(
    post,
    path = "/v1/templates/profiles",
    tag = "Templates",
    summary = "Create a template profile",
    description = "Creates a new template profile with field-to-template mappings.",
    request_body(content = CreateProfileRequest, description = "Profile definition"),
    responses(
        (status = 201, description = "Profile created", body = ProfileResponse),
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 409, description = "A profile with this name already exists in the scope", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn create_profile(
    State(state): State<AppState>,
    Json(req): Json<CreateProfileRequest>,
) -> impl IntoResponse {
    if let Err(e) = validate_template_name(&req.name) {
        return error_response(StatusCode::BAD_REQUEST, &e);
    }

    // Validate that all $ref templates exist.
    let gw = state.gateway.read().await;
    for (field_name, field) in &req.fields {
        if let TemplateProfileField::Ref { template_ref } = field {
            let tpl_key = format!("{}:{}:{template_ref}", req.namespace, req.tenant);
            if !gw.templates().contains_key(&tpl_key) {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("field '{field_name}' references unknown template '{template_ref}'"),
                );
            }
        }
    }

    let state_store = gw.state_store();

    // Check for duplicate.
    let idx_key = profile_index_key(&req.namespace, &req.tenant, &req.name);
    match state_store.get(&idx_key).await {
        Ok(Some(_)) => {
            return error_response(
                StatusCode::CONFLICT,
                &format!(
                    "a profile named '{}' already exists for namespace={} tenant={}",
                    req.name, req.namespace, req.tenant
                ),
            );
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        Ok(None) => {}
    }

    let now = Utc::now();
    let id = uuid::Uuid::new_v4().to_string();

    let profile = TemplateProfile {
        id: id.clone(),
        name: req.name,
        namespace: req.namespace,
        tenant: req.tenant,
        fields: req.fields,
        description: req.description,
        created_at: now,
        updated_at: now,
        labels: req.labels,
    };

    let key = profile_state_key(&id);
    let data = match serde_json::to_string(&profile) {
        Ok(d) => d,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("serialization error: {e}"),
            );
        }
    };
    if let Err(e) = state_store.set(&key, &data, None).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if let Err(e) = state_store.set(&idx_key, &id, None).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    drop(gw);

    let gw = state.gateway.read().await;
    gw.set_template_profile(profile.clone());

    let resp = profile_to_response(&profile);
    (StatusCode::CREATED, Json(serde_json::json!(resp))).into_response()
}

/// `GET /v1/templates/profiles` -- list template profiles.
#[utoipa::path(
    get,
    path = "/v1/templates/profiles",
    tag = "Templates",
    summary = "List template profiles",
    description = "Returns template profiles, optionally filtered by namespace and tenant.",
    params(ListTemplatesParams),
    responses(
        (status = 200, description = "Profile list", body = ListProfilesResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn list_profiles(
    State(state): State<AppState>,
    Query(params): Query<ListTemplatesParams>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let results = match state_store
        .scan_keys_by_kind(KeyKind::TemplateProfile)
        .await
    {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let mut profiles: Vec<ProfileResponse> = Vec::new();
    let mut skipped = 0usize;

    for (_key, value) in results {
        let Ok(prof) = serde_json::from_str::<TemplateProfile>(&value) else {
            continue;
        };

        if let Some(ref ns) = params.namespace
            && prof.namespace != *ns
        {
            continue;
        }
        if let Some(ref t) = params.tenant
            && prof.tenant != *t
        {
            continue;
        }

        if skipped < params.offset {
            skipped += 1;
            continue;
        }
        if profiles.len() >= params.limit {
            break;
        }

        profiles.push(profile_to_response(&prof));
    }

    let count = profiles.len();
    (
        StatusCode::OK,
        Json(serde_json::json!(ListProfilesResponse { profiles, count })),
    )
        .into_response()
}

/// `GET /v1/templates/profiles/{id}` -- get a single profile.
#[utoipa::path(
    get,
    path = "/v1/templates/profiles/{id}",
    tag = "Templates",
    summary = "Get template profile details",
    description = "Returns the full details of a template profile.",
    params(("id" = String, Path, description = "Profile ID")),
    responses(
        (status = 200, description = "Profile details", body = ProfileResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn get_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    match load_profile(state_store.as_ref(), &id).await {
        Ok(Some(prof)) => (
            StatusCode::OK,
            Json(serde_json::json!(profile_to_response(&prof))),
        )
            .into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, &format!("profile not found: {id}")),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

/// `PUT /v1/templates/profiles/{id}` -- update a template profile.
#[utoipa::path(
    put,
    path = "/v1/templates/profiles/{id}",
    tag = "Templates",
    summary = "Update a template profile",
    description = "Updates fields of an existing template profile.",
    params(("id" = String, Path, description = "Profile ID")),
    request_body(content = UpdateProfileRequest, description = "Fields to update"),
    responses(
        (status = 200, description = "Updated profile", body = ProfileResponse),
        (status = 400, description = "Validation error", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn update_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateProfileRequest>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let mut prof = match load_profile(state_store.as_ref(), &id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, &format!("profile not found: {id}"));
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    if let Some(ref fields) = req.fields {
        // Validate $ref templates exist.
        for (field_name, field) in fields {
            if let TemplateProfileField::Ref { template_ref } = field {
                let tpl_key = format!("{}:{}:{template_ref}", prof.namespace, prof.tenant);
                if !gw.templates().contains_key(&tpl_key) {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!(
                            "field '{field_name}' references unknown template '{template_ref}'"
                        ),
                    );
                }
            }
        }
        prof.fields.clone_from(fields);
    }
    if let Some(desc) = req.description {
        prof.description = Some(desc);
    }
    if let Some(labels) = req.labels {
        prof.labels = labels;
    }
    prof.updated_at = Utc::now();

    let key = profile_state_key(&id);
    let data = match serde_json::to_string(&prof) {
        Ok(d) => d,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("serialization error: {e}"),
            );
        }
    };
    if let Err(e) = state_store.set(&key, &data, None).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    drop(gw);

    let gw = state.gateway.read().await;
    gw.set_template_profile(prof.clone());

    let resp = profile_to_response(&prof);
    (StatusCode::OK, Json(serde_json::json!(resp))).into_response()
}

/// `DELETE /v1/templates/profiles/{id}` -- delete a template profile.
#[utoipa::path(
    delete,
    path = "/v1/templates/profiles/{id}",
    tag = "Templates",
    summary = "Delete a template profile",
    description = "Removes a template profile.",
    params(("id" = String, Path, description = "Profile ID")),
    responses(
        (status = 204, description = "Profile deleted"),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn delete_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let state_store = gw.state_store();

    let prof = match load_profile(state_store.as_ref(), &id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, &format!("profile not found: {id}"));
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    let key = profile_state_key(&id);
    if let Err(e) = state_store.delete(&key).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let idx_key = profile_index_key(&prof.namespace, &prof.tenant, &prof.name);
    let _ = state_store.delete(&idx_key).await;
    drop(gw);

    let gw = state.gateway.read().await;
    gw.remove_template_profile(&prof.namespace, &prof.tenant, &prof.name);

    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// Render preview handler
// ---------------------------------------------------------------------------

/// `POST /v1/templates/render` -- preview template rendering.
#[utoipa::path(
    post,
    path = "/v1/templates/render",
    tag = "Templates",
    summary = "Preview template rendering",
    description = "Renders a template profile against a test payload without dispatching.",
    request_body(content = RenderPreviewRequest, description = "Profile name and test payload"),
    responses(
        (status = 200, description = "Rendered output", body = RenderPreviewResponse),
        (status = 400, description = "Render error", body = ErrorResponse),
        (status = 404, description = "Profile not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
    )
)]
pub async fn render_preview(
    State(state): State<AppState>,
    Json(req): Json<RenderPreviewRequest>,
) -> impl IntoResponse {
    let gw = state.gateway.read().await;

    let profile_key = format!("{}:{}:{}", req.namespace, req.tenant, req.profile);
    let profile = match gw.template_profiles().get(&profile_key) {
        Some(p) => p.clone(),
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("profile not found: {}", req.profile),
            );
        }
    };

    // Gather scoped templates.
    let prefix = format!("{}:{}:", req.namespace, req.tenant);
    let scoped_templates: HashMap<String, Template> = gw
        .templates()
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(_, v)| (v.name.clone(), v.clone()))
        .collect();

    match acteon_gateway::template_engine::render_profile(&profile, &scoped_templates, &req.payload)
    {
        Ok(result) => (
            StatusCode::OK,
            Json(serde_json::json!(RenderPreviewResponse {
                rendered: result.fields,
            })),
        )
            .into_response(),
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}
