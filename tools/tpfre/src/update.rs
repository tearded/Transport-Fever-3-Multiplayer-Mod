//! One operator entry point; unknown builds retain diagnostics and stop before Cargo.
//! An exact, deliberately selected native bundle is still a reviewed input,
//! never inferred from matching signatures or from normalized function equality.

use crate::{archive, audit, build_gate};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{collections::BTreeMap, fs, io::Write, num::NonZeroUsize, path::Path, process::Command};

pub enum Input<'a> {
    Archive(&'a Path),
    Install {
        game: &'a Path,
        build: &'a str,
        executable: &'a str,
        steam_manifest: Option<&'a Path>,
    },
}

pub struct Options<'a> {
    pub old: &'a Path,
    pub input: Input<'a>,
    pub repository: &'a Path,
    pub output: &'a Path,
    pub cache: Option<&'a Path>,
    pub jobs: Option<NonZeroUsize>,
    pub check_only: bool,
}

#[derive(Serialize)]
struct Summary {
    format_version: u32,
    status: &'static str,
    stage: &'static str,
    reason: Option<String>,
    audit_review_required: Option<bool>,
    candidate_targets: Option<usize>,
    native_bundle: Option<String>,
    runtime_verified: bool,
    problems: Vec<Problem>,
}

#[derive(Serialize)]
struct Problem {
    stage: &'static str,
    reason: String,
}

fn save(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    writeln!(file)?;
    file.sync_all()?;
    Ok(())
}

pub fn run(options: &Options<'_>, out: &mut dyn Write) -> Result<i32> {
    run_with(options, out, |repo, args, log| {
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(log)?;
        let status = Command::new(build_gate::cargo_program())
            .args(args)
            .current_dir(repo)
            .stdout(file.try_clone()?)
            .stderr(file)
            .status()
            .context("run Cargo through the machine's configured wrapper")?;
        Ok(if status.success() { 0 } else { 1 })
    })
}

/// The injectable runner proves that no compilation follows failed checks/tests.
pub fn run_with(
    options: &Options<'_>,
    out: &mut dyn Write,
    mut runner: impl FnMut(&Path, &[String], &Path) -> Result<i32>,
) -> Result<i32> {
    let repository = options.repository.canonicalize().context("repository")?;
    ensure!(
        repository.join("Cargo.toml").is_file(),
        "repository has no Cargo.toml"
    );
    let old = options.old.canonicalize().context("baseline archive")?;
    ensure!(
        old.is_dir(),
        "baseline must be a complete archive, not an EXE"
    );
    let input = match &options.input {
        Input::Archive(path) => path.canonicalize().context("new archive")?,
        Input::Install { game, .. } => game.canonicalize().context("game install")?,
    };
    ensure!(input.is_dir(), "new input must be a directory");
    // These checks refuse existing outputs, traversal and Git/game destinations.
    archive::check_output_location(&old, options.output)?;
    archive::check_output_location(&input, options.output)?;
    archive::check_output_location(&repository, options.output)?;
    if let Some(cache) = options.cache {
        let probe = cache.join(".update-write-location-check");
        for root in [&old, &input, &repository] {
            archive::check_output_location(root, &probe)?;
        }
    }
    archive::check_output(&old, options.output)?;
    archive::check_output_location(&input, options.output)?;
    archive::check_output_location(&repository, options.output)?;
    fs::create_dir(options.output)?;
    let output = options.output.canonicalize()?;
    let mut summary = Summary {
        format_version: 1,
        status: "running",
        stage: "archive",
        reason: None,
        audit_review_required: None,
        candidate_targets: None,
        native_bundle: None,
        runtime_verified: false,
        problems: Vec::new(),
    };
    let result = pipeline(
        options,
        &repository,
        &old,
        &input,
        &output,
        &mut summary,
        out,
        &mut runner,
    );
    let code = match result {
        Ok(()) => {
            summary.status = if options.check_only {
                "static_checks_passed"
            } else {
                "built"
            };
            0
        }
        Err(error) => {
            summary.status = "blocked";
            summary.reason = Some(format!("{error:#}"));
            if summary.problems.is_empty() {
                summary.problems.push(Problem {
                    stage: summary.stage,
                    reason: format!("{error:#}"),
                });
            }
            writeln!(out, "STOP at {}: {error:#}", summary.stage)?;
            1
        }
    };
    save(&output.join("summary.json"), &summary)?;
    writeln!(
        out,
        "{}: reports in {} (runtime acceptance remains separate)",
        summary.status,
        output.display()
    )?;
    Ok(code)
}

fn stage(summary: &mut Summary, name: &'static str, out: &mut dyn Write) -> Result<()> {
    summary.stage = name;
    writeln!(out, "update: {name}")?;
    out.flush()?;
    Ok(())
}

// An unchanged EXE does not approve changed libraries or scripts automatically.
fn payload(manifest: &archive::Manifest) -> BTreeMap<&str, &str> {
    manifest
        .files
        .iter()
        .filter(|file| !file.path.starts_with("steam/"))
        .map(|file| (file.path.as_str(), file.sha256.as_str()))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn pipeline(
    options: &Options<'_>,
    repository: &Path,
    old: &Path,
    input: &Path,
    output: &Path,
    summary: &mut Summary,
    out: &mut dyn Write,
    runner: &mut impl FnMut(&Path, &[String], &Path) -> Result<i32>,
) -> Result<()> {
    stage(summary, "archive", out)?;
    let old_manifest = archive::verify(old).context("complete baseline archive")?;
    let new = match &options.input {
        Input::Archive(_) => input.to_path_buf(),
        Input::Install {
            build,
            executable,
            steam_manifest,
            ..
        } => {
            let snapshot = output.join("archive");
            archive::create(&archive::Options {
                game: input,
                out: &snapshot,
                build,
                executable,
                steam_manifest: *steam_manifest,
            })?;
            snapshot
        }
    };
    let new_manifest = archive::verify(&new).context("complete new archive")?;
    let old_payload = payload(&old_manifest);
    let new_payload = payload(&new_manifest);
    let changed = new_payload
        .iter()
        .filter(|(path, hash)| old_payload.get(*path).is_some_and(|old| old != *hash))
        .map(|(path, _)| *path)
        .collect::<Vec<_>>();
    let added = new_payload
        .keys()
        .filter(|path| !old_payload.contains_key(**path))
        .copied()
        .collect::<Vec<_>>();
    let removed = old_payload
        .keys()
        .filter(|path| !new_payload.contains_key(**path))
        .copied()
        .collect::<Vec<_>>();
    save(
        &output.join("files.json"),
        &serde_json::json!({"changed": changed, "added": added, "removed": removed}),
    )?;
    writeln!(
        out,
        "archive contents: {} changed, {} added, {} removed files",
        changed.len(),
        added.len(),
        removed.len()
    )?;
    let profiles = repository.join("profiles");
    stage(summary, "audit", out)?;
    let local_cache = output.join("indexes");
    let comparison = audit::compare(old, &new, &profiles, options.cache.unwrap_or(&local_cache))?;
    summary.audit_review_required = Some(comparison.review_required);
    save(&output.join("audit.json"), &comparison)?;
    let changed = comparison
        .targets
        .iter()
        .filter(|target| target.status != "matched")
        .count();
    let bodies = comparison
        .targets
        .iter()
        .filter(|target| target.normalized_function_equal != Some(true))
        .count();
    writeln!(
        out,
        "baseline comparison: {changed} changed signatures, {bodies} changed/unknown functions, {} changed scripts",
        comparison.scripts.changed.len()
    )?;
    // Capture the candidate report before the selected-bundle gate. A candidate
    // matching 143/143 is useful evidence, but cannot select/enable its native data.
    stage(summary, "signatures", out)?;
    match audit::verify(&new, &profiles) {
        Ok(signatures) => {
            summary.candidate_targets = Some(signatures.targets.len());
            save(&output.join("signatures.json"), &signatures)?;
            if signatures.review_required {
                summary.problems.push(Problem { stage: "signatures", reason: "candidate signatures failed; inspect signatures.json".into() });
            }
        }
        Err(error) => summary.problems.push(Problem { stage: "signatures", reason: format!("no valid exact-build candidate: {error:#}; inspect audit.json and prepare its profile") }),
    }
    stage(summary, "native-bundle", out)?;
    let verified = match build_gate::verify(&new, repository) {
        Ok(verified) => {
            save(&output.join("native-bundle.json"), &verified)?;
            summary.native_bundle = Some(verified.bundle.clone());
            if verified.report.review_required {
                summary.problems.push(Problem {
                    stage: "native-bundle",
                    reason: "selected native bundle has failed targets".into(),
                });
            }
            Some(verified)
        }
        Err(error) => {
            summary.problems.push(Problem { stage: "native-bundle", reason: format!("review and select the matching native bundle; signature success alone cannot enable an update: {error:#}") });
            None
        }
    };
    if comparison
        .old
        .as_ref()
        .is_some_and(|id| id.sha256 == comparison.new.sha256)
        && old_payload != new_payload
    {
        summary.problems.push(Problem { stage: "native-bundle", reason: "EXE unchanged but archived scripts/libraries changed; review those changes before reusing its native bundle (files.json)".into() });
    }
    if !summary.problems.is_empty() {
        for problem in &summary.problems {
            writeln!(out, "{}: {}", problem.stage, problem.reason)?;
        }
        summary.stage = summary.problems[0].stage;
        anyhow::bail!(
            "{} blocking checks require changes before any tests/build",
            summary.problems.len()
        );
    }
    let verified = verified.context("selected native bundle")?;
    // A selected, exact new bundle is an explicitly prepared/reviewed input.
    // The baseline audit remains diagnostic: old splices may intentionally no
    // longer match the newly reviewed bundle. Selection is never changed here.
    if options.check_only {
        return Ok(());
    }
    let jobs = options
        .jobs
        .map_or_else(Vec::new, |jobs| vec!["--jobs".into(), jobs.to_string()]);
    for (name, mut args) in [
        (
            "format",
            vec!["fmt", "--all", "--", "--check"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>(),
        ),
        (
            "clippy",
            vec!["clippy", "--workspace", "--all-targets", "--locked"]
                .into_iter()
                .map(String::from)
                .collect(),
        ),
        (
            "tests",
            vec!["test", "--workspace", "--locked", "--no-fail-fast"]
                .into_iter()
                .map(String::from)
                .collect(),
        ),
    ] {
        stage(summary, name, out)?;
        if name != "format" {
            args.extend(jobs.clone());
        }
        if name == "clippy" {
            args.extend(["--".into(), "-D".into(), "warnings".into()]);
        }
        let log = output.join(format!("{name}.log"));
        ensure!(
            runner(repository, &args, &log)? == 0,
            "{name} failed; inspect {}",
            log.display()
        );
    }
    stage(summary, "build", out)?;
    // Recheck the live archive and selected bytes after the tests; never accept
    // a stale report, a concurrent selection change or altered ABI data.
    let final_check = build_gate::verify(&new, repository)?;
    ensure!(
        verified.bundle == final_check.bundle
            && verified.bundle_files == final_check.bundle_files
            && verified.source_commit == final_check.source_commit
            && !final_check.report.review_required,
        "native bundle or commit changed during the checks; rerun the update pipeline"
    );
    let code = build_gate::build_with(&new, repository, options.jobs, out, |repo, args| {
        runner(repo, args, &output.join("build.log"))
    })?;
    ensure!(code == 0, "release build failed; inspect build.log");
    Ok(())
}
