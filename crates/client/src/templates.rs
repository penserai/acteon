use serde::{Deserialize, Serialize};

use crate::dispatch::ErrorResponse;
use crate::{ActeonClient, Error};

/// A reusable `MiniJinja` template stored in the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateInfo {
    /// Unique template ID.
    pub id: String,
    /// Template name (unique within namespace + tenant scope).
    pub name: String,
    /// Namespace this template belongs to.
    pub namespace: String,
    /// Tenant this template belongs to.
    pub tenant: String,
    /// Raw `MiniJinja` template content.
    pub content: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// When this template was created.
    pub created_at: String,
    /// When this template was last updated.
    pub updated_at: String,
    /// Arbitrary key-value labels.
    #[serde(default)]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// A template profile that maps payload fields to template content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateProfileInfo {
    /// Unique profile ID.
    pub id: String,
    /// Profile name (unique within namespace + tenant scope).
    pub name: String,
    /// Namespace this profile belongs to.
    pub namespace: String,
    /// Tenant this profile belongs to.
    pub tenant: String,
    /// Field-to-template mappings.
    pub fields: std::collections::HashMap<String, serde_json::Value>,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// When this profile was created.
    pub created_at: String,
    /// When this profile was last updated.
    pub updated_at: String,
    /// Arbitrary key-value labels.
    #[serde(default)]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Request to create a new template.
#[derive(Debug, Clone, Serialize)]
pub struct CreateTemplateRequest {
    /// Template name.
    pub name: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Raw `MiniJinja` template content.
    pub content: String,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arbitrary key-value labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Request to update an existing template.
#[derive(Debug, Default, Clone, Serialize)]
pub struct UpdateTemplateRequest {
    /// Updated template content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Updated description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Updated labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Request to create a new template profile.
#[derive(Debug, Clone, Serialize)]
pub struct CreateProfileRequest {
    /// Profile name.
    pub name: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Field-to-template mappings.
    pub fields: std::collections::HashMap<String, serde_json::Value>,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arbitrary key-value labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Request to update an existing template profile.
#[derive(Debug, Default, Clone, Serialize)]
pub struct UpdateProfileRequest {
    /// Updated field-to-template mappings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// Updated description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Updated labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Response from listing templates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListTemplatesResponse {
    /// List of templates.
    pub templates: Vec<TemplateInfo>,
    /// Total count of results.
    pub count: usize,
}

/// Response from listing template profiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListProfilesResponse {
    /// List of template profiles.
    pub profiles: Vec<TemplateProfileInfo>,
    /// Total count of results.
    pub count: usize,
}

/// Request to render a template preview.
#[derive(Debug, Clone, Serialize)]
pub struct RenderPreviewRequest {
    /// Profile name to render.
    pub profile: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Payload variables to use for rendering.
    pub payload: serde_json::Value,
}

/// Response from rendering a template preview.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderPreviewResponse {
    /// Rendered output keyed by field name.
    pub rendered: std::collections::HashMap<String, String>,
}

impl ActeonClient {
    /// Create a new template.
    pub async fn create_template(
        &self,
        req: &CreateTemplateRequest,
    ) -> Result<TemplateInfo, Error> {
        let url = format!("{}/v1/templates", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TemplateInfo>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            let error = response
                .json::<ErrorResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Err(Error::Api {
                code: error.code,
                message: error.message,
                retryable: error.retryable,
            })
        }
    }

    /// List templates, optionally filtered by namespace and tenant.
    pub async fn list_templates(
        &self,
        namespace: Option<&str>,
        tenant: Option<&str>,
    ) -> Result<ListTemplatesResponse, Error> {
        let url = format!("{}/v1/templates", self.base_url);

        let mut query: Vec<(&str, &str)> = Vec::new();
        if let Some(ns) = namespace {
            query.push(("namespace", ns));
        }
        if let Some(t) = tenant {
            query.push(("tenant", t));
        }

        let response = self
            .add_auth(self.client.get(&url))
            .query(&query)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ListTemplatesResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to list templates".to_string(),
            })
        }
    }

    /// Get a single template by ID.
    ///
    /// Returns `None` if not found.
    pub async fn get_template(&self, id: &str) -> Result<Option<TemplateInfo>, Error> {
        let url = format!("{}/v1/templates/{id}", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TemplateInfo>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get template: {id}"),
            })
        }
    }

    /// Update a template.
    pub async fn update_template(
        &self,
        id: &str,
        update: &UpdateTemplateRequest,
    ) -> Result<TemplateInfo, Error> {
        let url = format!("{}/v1/templates/{id}", self.base_url);

        let response = self
            .add_auth(self.client.put(&url))
            .json(update)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TemplateInfo>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Template not found: {id}"),
            })
        } else {
            let error = response
                .json::<ErrorResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Err(Error::Api {
                code: error.code,
                message: error.message,
                retryable: error.retryable,
            })
        }
    }

    /// Delete a template.
    pub async fn delete_template(&self, id: &str) -> Result<(), Error> {
        let url = format!("{}/v1/templates/{id}", self.base_url);

        let response = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NO_CONTENT {
            Ok(())
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Template not found: {id}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to delete template".to_string(),
            })
        }
    }

    /// Create a new template profile.
    pub async fn create_profile(
        &self,
        req: &CreateProfileRequest,
    ) -> Result<TemplateProfileInfo, Error> {
        let url = format!("{}/v1/templates/profiles", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TemplateProfileInfo>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            let error = response
                .json::<ErrorResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Err(Error::Api {
                code: error.code,
                message: error.message,
                retryable: error.retryable,
            })
        }
    }

    /// List template profiles, optionally filtered by namespace and tenant.
    pub async fn list_profiles(
        &self,
        namespace: Option<&str>,
        tenant: Option<&str>,
    ) -> Result<ListProfilesResponse, Error> {
        let url = format!("{}/v1/templates/profiles", self.base_url);

        let mut query: Vec<(&str, &str)> = Vec::new();
        if let Some(ns) = namespace {
            query.push(("namespace", ns));
        }
        if let Some(t) = tenant {
            query.push(("tenant", t));
        }

        let response = self
            .add_auth(self.client.get(&url))
            .query(&query)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ListProfilesResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to list template profiles".to_string(),
            })
        }
    }

    /// Get a single template profile by ID.
    ///
    /// Returns `None` if not found.
    pub async fn get_profile(&self, id: &str) -> Result<Option<TemplateProfileInfo>, Error> {
        let url = format!("{}/v1/templates/profiles/{id}", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TemplateProfileInfo>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get template profile: {id}"),
            })
        }
    }

    /// Update a template profile.
    pub async fn update_profile(
        &self,
        id: &str,
        update: &UpdateProfileRequest,
    ) -> Result<TemplateProfileInfo, Error> {
        let url = format!("{}/v1/templates/profiles/{id}", self.base_url);

        let response = self
            .add_auth(self.client.put(&url))
            .json(update)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TemplateProfileInfo>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Template profile not found: {id}"),
            })
        } else {
            let error = response
                .json::<ErrorResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Err(Error::Api {
                code: error.code,
                message: error.message,
                retryable: error.retryable,
            })
        }
    }

    /// Delete a template profile.
    pub async fn delete_profile(&self, id: &str) -> Result<(), Error> {
        let url = format!("{}/v1/templates/profiles/{id}", self.base_url);

        let response = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NO_CONTENT {
            Ok(())
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Template profile not found: {id}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to delete template profile".to_string(),
            })
        }
    }

    /// Render a template preview using the given profile and payload.
    pub async fn render_preview(
        &self,
        req: &RenderPreviewRequest,
    ) -> Result<RenderPreviewResponse, Error> {
        let url = format!("{}/v1/templates/render", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<RenderPreviewResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            let error = response
                .json::<ErrorResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Err(Error::Api {
                code: error.code,
                message: error.message,
                retryable: error.retryable,
            })
        }
    }
}
