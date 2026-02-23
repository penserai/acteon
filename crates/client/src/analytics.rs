pub use acteon_core::analytics::{
    AnalyticsBucket, AnalyticsInterval, AnalyticsMetric, AnalyticsQuery, AnalyticsResponse,
    AnalyticsTopEntry,
};

use crate::{ActeonClient, Error};

impl ActeonClient {
    /// Query aggregated action analytics.
    ///
    /// Sends a GET request to `/v1/analytics` with the query parameters
    /// derived from the given [`AnalyticsQuery`].
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, AnalyticsQuery, AnalyticsMetric, AnalyticsInterval};
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let query = AnalyticsQuery {
    ///     metric: AnalyticsMetric::Volume,
    ///     interval: AnalyticsInterval::Daily,
    ///     namespace: None,
    ///     tenant: Some("tenant-1".to_string()),
    ///     provider: None,
    ///     action_type: None,
    ///     outcome: None,
    ///     from: None,
    ///     to: None,
    ///     group_by: None,
    ///     top_n: None,
    /// };
    ///
    /// let response = client.query_analytics(&query).await?;
    /// println!("Total actions: {}", response.total_count);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn query_analytics(
        &self,
        query: &AnalyticsQuery,
    ) -> Result<AnalyticsResponse, Error> {
        let url = format!("{}/v1/analytics", self.base_url);

        // Build query params manually because AnalyticsQuery contains nested
        // types (enums, DateTime) that don't serialize cleanly via
        // serde_urlencoded.
        let mut params: Vec<(&str, String)> = Vec::new();

        // Metric (required) — serialize as snake_case.
        let metric_str = match query.metric {
            AnalyticsMetric::Volume => "volume",
            AnalyticsMetric::OutcomeBreakdown => "outcome_breakdown",
            AnalyticsMetric::TopActionTypes => "top_action_types",
            AnalyticsMetric::Latency => "latency",
            AnalyticsMetric::ErrorRate => "error_rate",
        };
        params.push(("metric", metric_str.to_string()));

        // Interval — serialize as snake_case.
        let interval_str = match query.interval {
            AnalyticsInterval::Hourly => "hourly",
            AnalyticsInterval::Daily => "daily",
            AnalyticsInterval::Weekly => "weekly",
            AnalyticsInterval::Monthly => "monthly",
        };
        params.push(("interval", interval_str.to_string()));

        // Optional string filters.
        if let Some(ref ns) = query.namespace {
            params.push(("namespace", ns.clone()));
        }
        if let Some(ref t) = query.tenant {
            params.push(("tenant", t.clone()));
        }
        if let Some(ref p) = query.provider {
            params.push(("provider", p.clone()));
        }
        if let Some(ref at) = query.action_type {
            params.push(("action_type", at.clone()));
        }
        if let Some(ref o) = query.outcome {
            params.push(("outcome", o.clone()));
        }
        if let Some(ref gb) = query.group_by {
            params.push(("group_by", gb.clone()));
        }

        // DateTime fields as RFC 3339 strings.
        if let Some(from) = query.from {
            params.push(("from", from.to_rfc3339()));
        }
        if let Some(to) = query.to {
            params.push(("to", to.to_rfc3339()));
        }

        // Numeric optional.
        if let Some(top_n) = query.top_n {
            params.push(("top_n", top_n.to_string()));
        }

        let response = self
            .add_auth(self.client.get(&url))
            .query(&params)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let resp = response
                .json::<AnalyticsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(resp)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to query analytics: {}", response.status()),
            })
        }
    }
}
