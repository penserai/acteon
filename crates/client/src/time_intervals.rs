//! Time intervals CRUD — thin HTTP wrappers for the `/v1/time-intervals` endpoints.
//!
//! Time intervals are tenant-scoped recurring schedules referenced by
//! rules through `mute_time_intervals` / `active_time_intervals` to gate
//! dispatch by wall-clock time. Mirrors Alertmanager's `time_intervals`.

pub use acteon_core::time_interval::{
    DayOfMonthRange, MonthRange, TimeInterval, TimeOfDayRange, TimeRange, WeekdayRange, YearRange,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ActeonClient, Error};

/// Time-of-day window in `HH:MM` form (matches the server API).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeOfDayInput {
    pub start: String,
    pub end: String,
}

/// One time range inside a [`CreateTimeIntervalRequest`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimeRangeInput {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub times: Vec<TimeOfDayInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weekdays: Vec<WeekdayRange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub days_of_month: Vec<DayOfMonthRange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub months: Vec<MonthRange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub years: Vec<YearRange>,
}

/// Request body for `POST /v1/time-intervals`.
#[derive(Debug, Clone, Serialize)]
pub struct CreateTimeIntervalRequest {
    pub name: String,
    pub namespace: String,
    pub tenant: String,
    #[serde(default)]
    pub time_ranges: Vec<TimeRangeInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Partial update body for `PUT /v1/time-intervals/{ns}/{tenant}/{name}`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateTimeIntervalRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_ranges: Option<Vec<TimeRangeInput>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Time interval shape returned from the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeIntervalResponse {
    pub name: String,
    pub namespace: String,
    pub tenant: String,
    pub time_ranges: Vec<TimeRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub matches_now: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListTimeIntervalsResponse {
    pub time_intervals: Vec<TimeIntervalResponse>,
    pub count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ListTimeIntervalsQuery {
    pub namespace: Option<String>,
    pub tenant: Option<String>,
}

impl ActeonClient {
    /// Create a new time interval.
    pub async fn create_time_interval(
        &self,
        req: &CreateTimeIntervalRequest,
    ) -> Result<TimeIntervalResponse, Error> {
        let url = format!("{}/v1/time-intervals", self.base_url);
        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if response.status().is_success() {
            response
                .json::<TimeIntervalResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("failed to create time interval: {}", response.status()),
            })
        }
    }

    /// List time intervals filtered by namespace and/or tenant.
    pub async fn list_time_intervals(
        &self,
        query: &ListTimeIntervalsQuery,
    ) -> Result<ListTimeIntervalsResponse, Error> {
        let url = format!("{}/v1/time-intervals", self.base_url);
        let mut req = self.add_auth(self.client.get(&url));
        if let Some(ref ns) = query.namespace {
            req = req.query(&[("namespace", ns)]);
        }
        if let Some(ref t) = query.tenant {
            req = req.query(&[("tenant", t)]);
        }
        let response = req
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if response.status().is_success() {
            response
                .json::<ListTimeIntervalsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("failed to list time intervals: {}", response.status()),
            })
        }
    }

    /// Fetch a single time interval. Returns `None` on 404.
    pub async fn get_time_interval(
        &self,
        namespace: &str,
        tenant: &str,
        name: &str,
    ) -> Result<Option<TimeIntervalResponse>, Error> {
        let url = format!(
            "{}/v1/time-intervals/{namespace}/{tenant}/{name}",
            self.base_url
        );
        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if response.status().is_success() {
            response
                .json::<TimeIntervalResponse>()
                .await
                .map(Some)
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("failed to get time interval: {}", response.status()),
            })
        }
    }

    /// Update an existing time interval.
    pub async fn update_time_interval(
        &self,
        namespace: &str,
        tenant: &str,
        name: &str,
        req: &UpdateTimeIntervalRequest,
    ) -> Result<TimeIntervalResponse, Error> {
        let url = format!(
            "{}/v1/time-intervals/{namespace}/{tenant}/{name}",
            self.base_url
        );
        let response = self
            .add_auth(self.client.put(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if response.status().is_success() {
            response
                .json::<TimeIntervalResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("failed to update time interval: {}", response.status()),
            })
        }
    }

    /// Delete a time interval.
    pub async fn delete_time_interval(
        &self,
        namespace: &str,
        tenant: &str,
        name: &str,
    ) -> Result<(), Error> {
        let url = format!(
            "{}/v1/time-intervals/{namespace}/{tenant}/{name}",
            self.base_url
        );
        let response = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("failed to delete time interval: {}", response.status()),
            })
        }
    }
}
