use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use acteon_simulation::scenario::{ScenarioManifest, ScenarioReport, evaluation, run};
use clap::Parser;

/// Run production-boundary scenarios, with strict invariants and semantic replay.
#[derive(Parser)]
struct Cli {
    #[arg(long, conflicts_with = "replay", required_unless_present = "replay")]
    manifest: Option<PathBuf>,
    /// Rerun a saved report and require matching evidence and runner provenance.
    #[arg(long)]
    replay: Option<PathBuf>,
    #[arg(long, default_value = "scenario-results")]
    output: PathBuf,
}

type Error = Box<dyn std::error::Error>;

fn replay_would_overwrite(source: &Path, output: &Path) -> Result<bool, Error> {
    let original = std::fs::canonicalize(source)?;
    for name in ["report.json", "manifest.json", "trace.jsonl", "junit.xml"] {
        let artifact = output.join(name);
        if !artifact.exists() {
            continue;
        }
        if original == std::fs::canonicalize(&artifact)? {
            return Ok(true);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let source = std::fs::metadata(&original)?;
            let target = std::fs::metadata(&artifact)?;
            if source.dev() == target.dev() && source.ino() == target.ino() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn read_json(path: &Path) -> Result<serde_json::Value, Error> {
    const LIMIT: u64 = 16 * 1024 * 1024;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err("manifest/report exceeds the 16 MiB limit".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), Error> {
    let mut output = BufWriter::new(std::fs::File::create(path)?);
    serde_json::to_writer_pretty(&mut output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

async fn legacy(input: serde_json::Value, replay: bool, output: &Path) -> Result<(), Error> {
    let previous: Option<ScenarioReport> = if replay {
        Some(serde_json::from_value(input.clone())?)
    } else {
        None
    };
    let manifest: ScenarioManifest = if let Some(previous) = &previous {
        previous.manifest.clone()
    } else {
        serde_json::from_value(input)?
    };
    let report = run(manifest).await?;
    std::fs::create_dir_all(output)?;
    write_json(&output.join("report.json"), &report)?;
    std::fs::write(output.join("junit.xml"), report.junit())?;
    let mut trace = BufWriter::new(std::fs::File::create(output.join("trace.jsonl"))?);
    for event in &report.trace {
        serde_json::to_writer(&mut trace, event)?;
        trace.write_all(b"\n")?;
    }
    trace.flush()?;
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
        output.display()
    );
    Ok(())
}

async fn suite(input: serde_json::Value, replay: bool, output: &Path) -> Result<(), Error> {
    let previous: Option<evaluation::EvaluationReport> = if replay {
        Some(serde_json::from_value(input.clone())?)
    } else {
        None
    };
    if let Some(previous) = &previous
        && previous.provenance != evaluation::Provenance::for_manifest(&previous.manifest)?
    {
        return Err(
            "evaluation runner provenance differs; replay with the original binary and Cargo.lock"
                .into(),
        );
    }
    if previous
        .as_ref()
        .is_some_and(|previous| !previous.consistent())
    {
        return Err("saved evaluation has inconsistent identities, scores, or evidence".into());
    }
    let manifest = if let Some(previous) = &previous {
        previous.manifest.clone()
    } else {
        serde_json::from_value(input)?
    };
    let report = evaluation::run(manifest).await?;
    std::fs::create_dir_all(output)?;
    write_json(&output.join("report.json"), &report)?;
    write_json(
        &output.join("manifest.json"),
        &serde_json::json!({"manifest":report.manifest,"provenance":report.provenance}),
    )?;
    std::fs::write(output.join("junit.xml"), report.junit())?;
    let mut trace = BufWriter::new(std::fs::File::create(output.join("trace.jsonl"))?);
    for trial in &report.results {
        for event in &trial.evidence.trace {
            serde_json::to_writer(
                &mut trace,
                &serde_json::json!({"trial":trial.trial,"seed":trial.seed,"event":event}),
            )?;
            trace.write_all(b"\n")?;
        }
    }
    trace.flush()?;
    if !report.passed() {
        return Err("one or more evaluation trials failed; see report.json".into());
    }
    if previous
        .as_ref()
        .is_some_and(|previous| !report.same_evidence(previous))
    {
        return Err("evaluation replay diverged (evidence or runner provenance); preserve the original binary and Cargo.lock".into());
    }
    println!(
        "{}/{} trials passed; safety gate failures: {}; worst score: {} basis points; evidence: {}",
        report.summary.passed,
        report.summary.trials,
        report.summary.safety_gate_failures,
        report.summary.worst_score_basis_points,
        output.display()
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::parse();
    let source = cli
        .replay
        .as_ref()
        .or(cli.manifest.as_ref())
        .expect("clap requires input");
    // A replay must never overwrite the evidence it is meant to compare.
    if cli.replay.is_some() && replay_would_overwrite(source, &cli.output)? {
        return Err("replay output must differ from the saved report directory".into());
    }
    let input = read_json(source)?;
    match input
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
    {
        Some(1) => legacy(input, cli.replay.is_some(), &cli.output).await,
        Some(2) => suite(input, cli.replay.is_some(), &cli.output).await,
        _ => Err("unsupported scenario schema_version (expected 1 or 2)".into()),
    }
}
