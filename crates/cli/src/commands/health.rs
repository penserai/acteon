use acteon_ops::OpsClient;

pub async fn run(ops: &OpsClient) -> anyhow::Result<()> {
    match ops.client().health().await {
        Ok(true) => {
            println!("Acteon gateway is healthy.");
            Ok(())
        }
        Ok(false) => {
            eprintln!("Acteon gateway returned unhealthy status.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Failed to reach gateway: {e}");
            std::process::exit(1);
        }
    }
}
