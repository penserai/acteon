use acteon_ops::OpsClient;
use tracing::{error, info};

pub async fn run(ops: &OpsClient) -> anyhow::Result<()> {
    match ops.client().health().await {
        Ok(true) => {
            info!("Acteon gateway is healthy");
            Ok(())
        }
        Ok(false) => {
            error!("Acteon gateway returned unhealthy status");
            std::process::exit(1);
        }
        Err(e) => {
            error!(error = %e, "Failed to reach gateway");
            std::process::exit(1);
        }
    }
}
