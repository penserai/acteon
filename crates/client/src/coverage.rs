//! Thin wrapper around the `GET /v1/rules/coverage` endpoint.
//!
//! All aggregation logic lives on the server (see `acteon_core::coverage` for
//! the shared types and `AnalyticsStore::rule_coverage` for the backend
//! aggregation). This module just serializes the query and parses the response.

pub use acteon_core::coverage::{CoverageAggregate, CoverageEntry, CoverageKey, CoverageReport};

use crate::{ActeonClient, Error};

/// Coverage filters within the authenticated context.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoverageQuery {
    /// Inclusive start time.
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    /// Inclusive end time.
    pub to: Option<chrono::DateTime<chrono::Utc>>,
}

impl ActeonClient {
    /// Analyze rule coverage by querying the server's aggregation endpoint.
    ///
    /// The server groups audit records by `(namespace, tenant, provider,
    /// action_type, matched_rule)` and cross-references the result with the
    /// currently-loaded rule set. Only the aggregated report is returned —
    /// the client never receives raw audit records.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, CoverageQuery};
    /// use chrono::{Duration, Utc};
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let query = CoverageQuery {
    ///     from: Some(Utc::now() - Duration::hours(24)),
    ///     ..Default::default()
    /// };
    /// let report = client.rules_coverage(&query).await?;
    /// println!("Uncovered combinations: {}", report.uncovered);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn rules_coverage(&self, query: &CoverageQuery) -> Result<CoverageReport, Error> {
        let url = format!("{}/v1/rules/coverage", self.base_url);

        let mut req = self.add_auth(self.client.get(&url));
        if let Some(from) = query.from {
            req = req.query(&[("from", from.to_rfc3339())]);
        }
        if let Some(to) = query.to {
            req = req.query(&[("to", to.to_rfc3339())]);
        }

        let response = req
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<CoverageReport>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get rule coverage: {}", response.status()),
            })
        }
    }
}
