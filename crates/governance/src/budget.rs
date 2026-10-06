//! Shared root accounting at the same CAS boundary as effect starts.
use serde::{Deserialize, Serialize};

use crate::{
    AuthorityCoordinator, AuthorityStamp, CONTROL_BYTE_RESERVE, CONTROL_RECORD_RESERVE,
    CoordinationError, RETRIES, valid_text,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootBudgetLimits {
    pub max_units: u64,
    pub max_concurrent: u64,
    pub deadline_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootBudget {
    /// Root owner is checked as an additional revocation ancestor.
    pub owner_subject: String,
    /// Immutable admitted recipient context; protects sponsored IDs from rebinding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_context: Option<acteon_core::ExecutionContextReference>,
    pub limits: RootBudgetLimits,
    /// Units are spent once per registered attempt, including retries. Known
    /// settlement releases concurrency, not spent call units.
    pub spent_units: u64,
    pub active_attempts: u64,
    /// Permanent execution-instance fence. Settlement remains available.
    #[serde(default)]
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootReservation {
    pub root_id: String,
    pub units: u64,
}

impl AuthorityCoordinator {
    /// Trusted adapter creates a bounded root allocation from independently
    /// evaluated permits. Replay observes it; it never increases its limits.
    /// This is not aggregate team funding or authorization of delegated lineage.
    pub async fn create_root_budget(
        &self,
        root_id: &str,
        owner_subject: &str,
        limits: RootBudgetLimits,
        evaluated_authority: &AuthorityStamp,
        now_ms: i64,
    ) -> Result<RootBudget, CoordinationError> {
        if !valid_text(root_id)
            || !valid_text(owner_subject)
            || limits.max_units == 0
            || limits.max_concurrent == 0
            || limits.deadline_ms <= 0
            || now_ms < 0
        {
            return Err(CoordinationError::Invalid("root budget fields".into()));
        }
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            if evaluated_authority.incarnation != state.incarnation {
                return Err(CoordinationError::StaleAuthority);
            }
            if state.budget_parents.contains_key(root_id) {
                return Err(CoordinationError::Conflict);
            }
            if let Some(existing) = state.roots.get(root_id) {
                if existing.owner_subject != owner_subject || existing.limits != limits {
                    return Err(CoordinationError::Conflict);
                }
                return Ok(existing.clone());
            }
            if state.stamp() != *evaluated_authority {
                return Err(CoordinationError::StaleAuthority);
            }
            if state.revoked_subjects.contains(owner_subject) {
                return Err(CoordinationError::Restricted);
            }
            if limits.deadline_ms <= now_ms {
                return Err(CoordinationError::DeadlineExceeded);
            }
            if limits.max_concurrent > u64::try_from(state.limits.max_active).unwrap_or(u64::MAX)
                || state.record_count() >= state.limits.max_records - CONTROL_RECORD_RESERVE
            {
                return Err(CoordinationError::Capacity);
            }
            let root = RootBudget {
                owner_subject: owner_subject.into(),
                accepted_context: None,
                limits: limits.clone(),
                spent_units: 0,
                active_attempts: 0,
                cancelled: false,
            };
            state.roots.insert(root_id.into(), root.clone());
            if Self::encode(&state)?.len() > state.limits.max_bytes - CONTROL_BYTE_RESERVE {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(root);
            }
        }
        Err(CoordinationError::Contention)
    }
}

/// Trusted same-actor child allocation. Parent identity and payer come from
/// verified provenance; allocating a budget does not authorize an effect.
pub struct ChildBudgetAdmission<'a> {
    pub parent: &'a crate::context::VerifiedExecutionContext,
    pub permits: &'a [crate::permit::PermitReference],
    pub execution_id: uuid::Uuid,
    pub limits: RootBudgetLimits,
    pub clock: &'a dyn acteon_time::Clock,
}

pub const MAX_BUDGET_DEPTH: usize = 16;
pub const MAX_ROOT_DESCENDANTS: usize = 128;

impl AuthorityCoordinator {
    pub async fn create_child_budget(
        &self,
        admission: ChildBudgetAdmission<'_>,
    ) -> Result<RootBudget, CoordinationError> {
        let child_id = admission.execution_id.to_string();
        let parent_id = admission.parent.execution_id().to_string();
        if admission.execution_id.is_nil()
            || child_id == parent_id
            || admission.limits.max_units == 0
            || admission.limits.max_concurrent == 0
        {
            return Err(CoordinationError::Invalid("child budget fields".into()));
        }
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            let now = admission.clock.now().timestamp_millis();
            admission
                .parent
                .validate_inheritance(&state, admission.permits, now)?;
            let parent = state
                .roots
                .get(&parent_id)
                .ok_or(CoordinationError::Conflict)?;
            if admission.limits.max_units > parent.limits.max_units
                || admission.limits.max_concurrent > parent.limits.max_concurrent
                || admission.limits.deadline_ms > parent.limits.deadline_ms
                || admission.limits.deadline_ms <= now
            {
                return Err(CoordinationError::Invalid(
                    "child exceeds parent budget".into(),
                ));
            }
            if let Some(existing) = state.roots.get(&child_id) {
                if existing.owner_subject != admission.parent.principal().id()
                    || existing.limits != admission.limits
                    || state.budget_parents.get(&child_id) != Some(&parent_id)
                {
                    return Err(CoordinationError::Conflict);
                }
                return Ok(existing.clone());
            }
            let path = budget_path(&state, &parent_id)?;
            if path.iter().any(|id| state.roots[id].cancelled) {
                return Err(CoordinationError::Restricted);
            }
            if path.len() >= MAX_BUDGET_DEPTH {
                return Err(CoordinationError::Capacity);
            }
            let root_id = path.last().ok_or(CoordinationError::Conflict)?;
            let descendants = state
                .budget_parents
                .keys()
                .filter(|id| budget_path(&state, id).is_ok_and(|p| p.last() == Some(root_id)))
                .count();
            if descendants >= MAX_ROOT_DESCENDANTS
                || state.record_count() + 2 > state.limits.max_records - CONTROL_RECORD_RESERVE
            {
                return Err(CoordinationError::Capacity);
            }
            let child = RootBudget {
                owner_subject: admission.parent.principal().id().into(),
                accepted_context: None,
                limits: admission.limits.clone(),
                spent_units: 0,
                active_attempts: 0,
                cancelled: false,
            };
            state
                .budget_parents
                .insert(child_id.clone(), parent_id.clone());
            state.roots.insert(child_id.clone(), child.clone());
            if Self::encode(&state)?.len() > state.limits.max_bytes - CONTROL_BYTE_RESERVE {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(child);
            }
        }
        Err(CoordinationError::Contention)
    }
}

/// Leaf through root, with bounded, cycle-free immutable parent links.
pub(crate) fn budget_path(
    state: &crate::CoordinatorSnapshot,
    leaf: &str,
) -> Result<Vec<String>, CoordinationError> {
    if !state.roots.contains_key(leaf) {
        return Err(CoordinationError::Conflict);
    }
    let mut path = Vec::new();
    let mut current = leaf;
    loop {
        if path.len() >= MAX_BUDGET_DEPTH
            || path.iter().any(|id| id == current)
            || !state.roots.contains_key(current)
        {
            return Err(CoordinationError::Invalid("invalid budget ancestry".into()));
        }
        path.push(current.to_owned());
        let Some(parent) = state.budget_parents.get(current) else {
            return Ok(path);
        };
        current = parent;
    }
}

pub(crate) fn valid_budget_links(state: &crate::CoordinatorSnapshot) -> bool {
    if !state.budget_parents.is_empty() && state.purpose != crate::ScopePurpose::Execution {
        return false;
    }
    let mut descendants = std::collections::BTreeMap::<String, usize>::new();
    for (child_id, parent_id) in &state.budget_parents {
        let (Some(child), Some(parent)) = (state.roots.get(child_id), state.roots.get(parent_id))
        else {
            return false;
        };
        if child.limits.max_units > parent.limits.max_units
            || child.limits.max_concurrent > parent.limits.max_concurrent
            || child.limits.deadline_ms > parent.limits.deadline_ms
        {
            return false;
        }
        let Ok(path) = budget_path(state, child_id) else {
            return false;
        };
        let Some(root) = path.last() else {
            return false;
        };
        let count = descendants.entry(root.clone()).or_default();
        *count += 1;
        if *count > MAX_ROOT_DESCENDANTS {
            return false;
        }
    }
    true
}

// All ledgers and the new start are committed at one CAS boundary.
pub(crate) fn reserve_root(
    state: &mut crate::CoordinatorSnapshot,
    reservation: &RootReservation,
    now_ms: i64,
) -> Result<(), CoordinationError> {
    let path = check_root_reservation(state, reservation, now_ms)?;
    for id in path {
        let budget = state
            .roots
            .get_mut(&id)
            .ok_or(CoordinationError::Conflict)?;
        budget.spent_units += reservation.units;
        budget.active_attempts += 1;
    }
    Ok(())
}

/// Advisory capacity check using the same constraints as atomic reservation.
/// This does not reserve capacity or establish permission for an effect.
pub(crate) fn check_root_reservation(
    state: &crate::CoordinatorSnapshot,
    reservation: &RootReservation,
    now_ms: i64,
) -> Result<Vec<String>, CoordinationError> {
    let path = budget_path(state, &reservation.root_id)?;
    // Check every ancestor before mutating any counter.
    for id in &path {
        let budget = &state.roots[id];
        if budget.cancelled || state.revoked_subjects.contains(&budget.owner_subject) {
            return Err(CoordinationError::Restricted);
        }
        if now_ms >= budget.limits.deadline_ms {
            return Err(CoordinationError::DeadlineExceeded);
        }
        if budget.active_attempts >= budget.limits.max_concurrent {
            return Err(CoordinationError::ConcurrencyExhausted);
        }
        if budget
            .spent_units
            .checked_add(reservation.units)
            .is_none_or(|n| n > budget.limits.max_units)
        {
            return Err(CoordinationError::BudgetExhausted);
        }
    }
    Ok(path)
}

pub(crate) fn release_concurrency(
    state: &mut crate::CoordinatorSnapshot,
    reservation: &RootReservation,
) -> Result<(), CoordinationError> {
    let path = budget_path(state, &reservation.root_id)?;
    if path.iter().any(|id| state.roots[id].active_attempts == 0) {
        return Err(CoordinationError::Conflict);
    }
    for id in path {
        state
            .roots
            .get_mut(&id)
            .ok_or(CoordinationError::Conflict)?
            .active_attempts -= 1;
    }
    Ok(())
}

pub(crate) fn current_limits_allow(
    state: &crate::CoordinatorSnapshot,
    leaf: &str,
    limits: &RootBudgetLimits,
    units: u64,
) -> Result<bool, CoordinationError> {
    Ok(budget_path(state, leaf)?.iter().all(|id| {
        let budget = &state.roots[id];
        !budget.cancelled
            && budget
                .spent_units
                .checked_add(units)
                .is_some_and(|n| n <= limits.max_units)
            && budget.active_attempts < limits.max_concurrent
    }))
}
