//! One search contract for all backends; arbitrary keys never become backend mappings.
use crate::{Error, Result, TimelineChange, TimelineEvent, TimelineQuery, TimelineScope};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchFields {
    pub exact: Vec<String>,
    pub paths: Vec<String>,
    pub tokens: Vec<String>,
    pub keyed_tokens: Vec<String>,
}
pub fn tokens(text: &str) -> Vec<String> {
    let mut result: Vec<_> = text.unicode_words().map(str::to_lowercase).collect();
    result.sort();
    result.dedup();
    result
}
pub fn exact(path: &str, value: &Value) -> String {
    crate::digest(&(path, value))
}
pub fn keyed_token(path: &str, token: &str) -> String {
    crate::digest(&(path, token))
}
fn walk(value: &Value, path: &str, depth: usize, fields: &mut SearchFields) -> Result<()> {
    if path.len() > 4096 {
        return Err(Error::Invalid("attribute path exceeds 4096 bytes".into()));
    }
    if depth > 16 {
        return Err(Error::Invalid("attributes exceed depth 16".into()));
    }
    if !path.is_empty() {
        fields.paths.push(path.to_owned());
    }
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let key = key.replace('~', "~0").replace('/', "~1");
                walk(value, &format!("{path}/{key}"), depth + 1, fields)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                walk(value, path, depth + 1, fields)?;
            }
        }
        scalar => {
            if fields.exact.len() >= 1000 {
                return Err(Error::Invalid(
                    "attributes exceed 1000 scalar leaves".into(),
                ));
            }
            fields.exact.push(exact(path, scalar));
            if let Value::String(text) = scalar {
                for token in tokens(text) {
                    fields.keyed_tokens.push(keyed_token(path, &token));
                    fields.tokens.push(token);
                }
            }
        }
    }
    Ok(())
}
impl SearchFields {
    pub fn from_event(event: &crate::EventBody) -> Result<Self> {
        let mut result = Self::default();
        walk(&event.attributes, "", 0, &mut result)?;
        crate::predicate::index_attributes(&event.attributes, "", &mut result);
        if let Some(message) = &event.message {
            result.tokens.extend(tokens(message));
        }
        for values in [
            &mut result.exact,
            &mut result.paths,
            &mut result.tokens,
            &mut result.keyed_tokens,
        ] {
            values.sort();
            values.dedup();
        }
        if result.tokens.iter().any(|token| token.len() > 8192) {
            return Err(Error::Invalid(
                "a searchable word exceeds 8192 bytes".into(),
            ));
        }
        Ok(result)
    }
}
pub fn validate_scope(scope: &TimelineScope) -> Result<()> {
    for s in [&scope.namespace, &scope.tenant] {
        if s.is_empty() || s.len() > 256 || s.chars().any(char::is_control) {
            return Err(Error::Invalid("invalid namespace or tenant".into()));
        }
    }
    Ok(())
}
pub fn validate_change(change: &TimelineChange) -> Result<()> {
    if change.operation == crate::ChangeOperation::Rejected || change.source.starts_with("_acteon_")
    {
        return Err(Error::Invalid("reserved ledger operation or source".into()));
    }
    if change.schema_version != 1 || change.revision == 0 || change.revision > i64::MAX as u64 {
        return Err(Error::Invalid(
            "schemaVersion must be 1 and revision must be a positive signed 64-bit integer".into(),
        ));
    }
    for s in [&change.change_id, &change.source, &change.event_id] {
        if s.is_empty() || s.len() > 256 || s.chars().any(char::is_control) {
            return Err(Error::Invalid("invalid change identity".into()));
        }
    }
    if serde_json::to_vec(change)?.len() > 256 * 1024 {
        return Err(Error::Invalid("change exceeds 256 KiB".into()));
    }
    if (change.operation == crate::ChangeOperation::Delete) != change.after.is_none() {
        return Err(Error::Invalid(
            "delete requires no after image; other operations require a complete after image"
                .into(),
        ));
    }
    for event in change.before.iter().chain(change.after.iter()) {
        if event.event_group.is_empty()
            || event.event_group.len() > 256
            || event.event_type.is_empty()
            || event.event_type.len() > 256
            || !event.attributes.is_object()
        {
            return Err(Error::Invalid("eventGroup/eventType must be nonempty, at most 256 bytes; attributes must be an object".into()));
        }
        if event
            .occurred_at
            .timestamp_nanos_opt()
            .is_none_or(|nanos| nanos < 0)
        {
            return Err(Error::Invalid("occurredAt must be within the shared backend nanosecond timestamp range (1970–2262)".into()));
        }
        SearchFields::from_event(event)?;
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cursor {
    pub fingerprint: String,
    pub occurred_at: DateTime<Utc>,
    pub source: String,
    pub event_id: String,
}
pub struct PreparedQuery {
    pub(crate) predicate: crate::predicate::Predicate,
    pub limit: usize,
    pub ascending: bool,
    pub fingerprint: String,
    pub cursor: Option<Cursor>,
}
impl PreparedQuery {
    pub fn new(scope: &TimelineScope, query: &TimelineQuery) -> Result<Self> {
        validate_scope(scope)?;
        let limit = query.limit.unwrap_or(50);
        if !(1..=1000).contains(&limit)
            || query.equals.len() + query.exists.len() > 64
            || query.from.zip(query.to).is_some_and(|(from, to)| from > to)
        {
            return Err(Error::Invalid("invalid query bounds".into()));
        }
        for path in query
            .equals
            .iter()
            .map(|e| &e.path)
            .chain(query.exists.iter())
            .chain(query.text.iter().filter_map(|t| t.path.as_ref()))
        {
            if !path.starts_with('/') || path.len() > 4096 {
                return Err(Error::Invalid(
                    "attribute paths must be JSON Pointers".into(),
                ));
            }
        }
        if query
            .equals
            .iter()
            .any(|e| e.value.is_object() || e.value.is_array())
        {
            return Err(Error::Invalid("equality requires scalar values".into()));
        }
        if let Some(text) = &query.text
            && (text.text.len() > 4096 || tokens(&text.text).is_empty())
        {
            return Err(Error::Invalid(
                "text query requires searchable terms, at most 4096 bytes".into(),
            ));
        }
        let mut normalized = query.clone();
        normalized.cursor = None;
        let fingerprint = crate::digest(&(scope, normalized));
        let cursor = query
            .cursor
            .as_ref()
            .map(|cursor| -> Result<Cursor> {
                if cursor.len() > 8192 {
                    return Err(Error::Invalid("invalid cursor".into()));
                }
                let bytes = URL_SAFE_NO_PAD
                    .decode(cursor)
                    .map_err(|_| Error::Invalid("invalid cursor".into()))?;
                let decoded: Cursor = serde_json::from_slice(&bytes)
                    .map_err(|_| Error::Invalid("invalid cursor".into()))?;
                if decoded.fingerprint != fingerprint {
                    return Err(Error::Invalid(
                        "cursor does not match scope and query".into(),
                    ));
                }
                Ok(decoded)
            })
            .transpose()?;
        Ok(Self {
            predicate: crate::predicate::prepare(query)?,
            limit: limit as usize,
            ascending: query.ascending.unwrap_or(query.event_group.is_some()),
            fingerprint,
            cursor,
        })
    }
    pub fn page(&self, mut events: Vec<TimelineEvent>) -> crate::TimelinePage {
        let more = events.len() > self.limit;
        events.truncate(self.limit);
        let next_cursor = events.last().filter(|_| more).map(|e| {
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&Cursor {
                    fingerprint: self.fingerprint.clone(),
                    occurred_at: e.event.occurred_at,
                    source: e.source.clone(),
                    event_id: e.event_id.clone(),
                })
                .expect("cursor serializes"),
            )
        });
        crate::TimelinePage {
            events,
            next_cursor,
        }
    }
}
pub fn matches(
    event: &TimelineEvent,
    fields: &SearchFields,
    _q: &TimelineQuery,
    p: &PreparedQuery,
    now: DateTime<Utc>,
) -> bool {
    if event.expires_at <= now || !p.predicate.matches(event, fields) {
        return false;
    }
    if let Some(c) = &p.cursor {
        let order = (event.event.occurred_at, &event.source, &event.event_id).cmp(&(
            c.occurred_at,
            &c.source,
            &c.event_id,
        ));
        if !(if p.ascending {
            order.is_gt()
        } else {
            order.is_lt()
        }) {
            return false;
        }
    }
    true
}
