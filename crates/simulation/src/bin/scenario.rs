use std::path::PathBuf;

use acteon_simulation::scenario::{ScenarioManifest, ScenarioReport, run};
use clap::Parser;

/// Run production-boundary scenarios, with strict invariants and semantic replay.
#[derive(Parser)]
struct Cli {
    #[arg(long, conflicts_with = "replay", required_unless_present = "replay")]
    manifest: Option<PathBuf>,
    /// Rerun a saved report's manifest and compare invariant and trace evidence.
    #[arg(long)]
    replay: Option<PathBuf>,
    #[arg(long, default_value = "scenario-results")]
    output: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let previous: Option<ScenarioReport> = cli
        .replay
        .map(|path| -> Result<_, Box<dyn std::error::Error>> {
            Ok(serde_json::from_slice(&std::fs::read(path)?)?)
        })
        .transpose()?;
    let manifest: ScenarioManifest = if let Some(previous) = &previous {
        previous.manifest.clone()
    } else {
        serde_json::from_slice(&std::fs::read(
            cli.manifest.expect("clap requires manifest"),
        )?)?
    };
    let report = run(manifest).await?;
    std::fs::create_dir_all(&cli.output)?;
    std::fs::write(
        cli.output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    std::fs::write(cli.output.join("junit.xml"), report.junit())?;
    let trace = report
        .trace
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    std::fs::write(cli.output.join("trace.jsonl"), format!("{trace}\n"))?;
    if !report.passed() {
        return Err("one or more scenario invariants failed; see report.json".into());
    }
    if previous
        .as_ref()
        .is_some_and(|previous| !report.same_evidence(previous))
    {
        return Err("semantic replay diverged; compare saved and new reports".into());
    }
    println!(
        "{} invariants passed; evidence: {}",
        report.invariants.len(),
        cli.output.display()
    );
    Ok(())
}
