//! Test providers for simulation testing.
//!
//! This module provides mock providers that can record calls, simulate
//! failures, and control response behavior for testing purposes.

mod failing;
mod recording;

pub use failing::{FailingProvider, FailureType};
pub use recording::{CapturedCall, FailureMode, RecordingProvider};
