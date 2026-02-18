//! AWS service providers for the Acteon action gateway.
//!
//! This crate provides feature-gated integrations with AWS services:
//!
//! - **SNS** (`sns` feature) — Publish messages to SNS topics
//! - **Lambda** (`lambda` feature) — Invoke Lambda functions
//! - **`EventBridge`** (`eventbridge` feature) — Put events to `EventBridge`
//! - **SQS** (`sqs` feature) — Send messages to SQS queues
//! - **SES** (`ses` feature) — Send emails via SES v2
//! - **S3** (`s3` feature) — Put/get/delete objects in S3 buckets
//!
//! All providers share a common [`AwsBaseConfig`](config::AwsBaseConfig) for
//! region, endpoint override, and optional STS assume-role credentials.

pub mod auth;
pub mod config;
pub mod error;

#[cfg(feature = "sns")]
pub mod sns;

#[cfg(feature = "lambda")]
pub mod lambda;

#[cfg(feature = "eventbridge")]
pub mod eventbridge;

#[cfg(feature = "sqs")]
pub mod sqs;

#[cfg(feature = "ses")]
pub mod ses;

#[cfg(feature = "s3")]
pub mod s3;

// Re-exports for convenience.
pub use config::AwsBaseConfig;
pub use error::AwsProviderError;

#[cfg(feature = "sns")]
pub use sns::{SnsConfig, SnsProvider};

#[cfg(feature = "lambda")]
pub use lambda::{LambdaConfig, LambdaProvider};

#[cfg(feature = "eventbridge")]
pub use eventbridge::{EventBridgeConfig, EventBridgeProvider};

#[cfg(feature = "sqs")]
pub use sqs::{SqsConfig, SqsProvider};

#[cfg(feature = "ses")]
pub use ses::{SesClient, SesConfig};

#[cfg(feature = "s3")]
pub use s3::{S3Config, S3Provider};
