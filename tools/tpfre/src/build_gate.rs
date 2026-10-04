//! Mandatory static check before the local update build or release packaging.
//! Inputs stay private; the report contains identities and checked profile bytes.

use crate::{archive, audit};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{fs, io::Write, num::NonZeroUsize, path::Path, process::Command};
use tpf3mp_hookcore::{bundle::Bundle, profile::Profile};

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct BundleFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Serialize)]
pub struct Verification {
    pub bundle: String,
    pub bundle_files: Vec<BundleFile>,
    pub source_commit: Option<String>,
    pub source_dirty: Option<bool>,
    #[serde(flatten)]
    pub report: audit::Report,
}

fn bundle_files(bundle: &Bundle) -> Result<Vec<BundleFile>> {
    let mut directories = vec![bundle.directory.clone()];
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            ensure!(!kind.is_symlink(), "linked native bundle input refused");
            let path = entry.path();
            if kind.is_dir() {
                directories.push(path);
            } else {
                ensure!(kind.is_file(), "non-file native bundle input refused");
                files.push(BundleFile {
                    path: path
                        .strip_prefix(&bundle.directory)?
                        .to_string_lossy()
                        .replace('\\', "/"),
                    sha256: Sha256::digest(fs::read(&path)?)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect(),
                });
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Only a complete private archive and the compiled bundle's exact profile.
pub fn verify(snapshot: &Path, repository: &Path) -> Result<Verification> {
    let repository = repository.canonicalize().context("repository directory")?;
    ensure!(
        repository.join("Cargo.toml").is_file(),
        "repository has no Cargo.toml"
    );
    ensure!(
        snapshot.is_dir(),
        "build verification requires a complete source archive, not an EXE alone"
    );
    let snapshot = snapshot.canonicalize()?;
    ensure!(
        !snapshot.starts_with(&repository),
        "game archive must stay outside the repository"
    );
    let profiles = repository.join("profiles");
    let selected = Bundle::selected(&profiles).context("selected native bundle")?;
    let before = bundle_files(&selected)?;
    let profile = Profile::from_toml(&fs::read_to_string(selected.directory.join("hooks.toml"))?)?;
    let mut report = audit::verify_selected(&snapshot, profile)?;
    report.kind = "selected_build_verification";
    let after = Bundle::selected(&profiles)?;
    ensure!(
        selected.name == after.name
            && selected.directory == after.directory
            && before == bundle_files(&after)?,
        "native bundle changed during verification"
    );
    // Recheck the whole snapshot after scanning, including sources/libraries.
    archive::verify(&snapshot)?;
    let source_commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repository)
        .output()
        .ok()
        .filter(|result| result.status.success())
        .and_then(|result| String::from_utf8(result.stdout).ok())
        .map(|commit| commit.trim().to_owned());
    let source_dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repository)
        .output()
        .ok()
        .filter(|result| result.status.success())
        .map(|result| !result.stdout.is_empty());
    Ok(Verification {
        bundle: selected.name,
        bundle_files: before,
        source_commit,
        source_dirty,
        report,
    })
}

pub fn print(verification: &Verification, json: bool, out: &mut dyn Write) -> Result<i32> {
    if json {
        serde_json::to_writer_pretty(&mut *out, verification)?;
        writeln!(out)?;
        Ok(i32::from(verification.report.review_required))
    } else {
        writeln!(out, "native bundle {}", verification.bundle)?;
        audit::print(&verification.report, false, out)
    }
}

/// Verify now, then invoke the fixed package build. Old reports are never input.
/// The runner is injectable so tests prove Cargo is not invoked after refusal.
pub fn build_with(
    snapshot: &Path,
    repository: &Path,
    jobs: Option<NonZeroUsize>,
    out: &mut dyn Write,
    runner: impl FnOnce(&Path, &[String]) -> Result<i32>,
) -> Result<i32> {
    let verification = verify(snapshot, repository)?;
    if print(&verification, false, out)? != 0 {
        return Ok(1);
    }
    out.flush()?;
    let mut args: Vec<String> = [
        "build",
        "--release",
        "--locked",
        "-p",
        "tpf3mp-launcher",
        "-p",
        "tpf3mp-agent",
        "-p",
        "tpf3mp-server",
        "-p",
        "tpf3mp-hook",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    if let Some(jobs) = jobs {
        args.extend(["--jobs".into(), jobs.to_string()]);
    }
    runner(&repository.canonicalize()?, &args)
}

pub fn build(
    snapshot: &Path,
    repository: &Path,
    jobs: Option<NonZeroUsize>,
    out: &mut dyn Write,
) -> Result<i32> {
    build_with(snapshot, repository, jobs, out, |repository, args| {
        // Respect a machine's quiet-cargo queue/cap when it is installed.
        // CI machines without that wrapper use their normal Cargo executable.
        let cargo = cargo_program();
        let status = Command::new(cargo)
            .args(args)
            .current_dir(repository)
            .status()
            .context("start Cargo after game verification")?;
        Ok(if status.success() { 0 } else { 1 })
    })
}

pub(crate) fn cargo_program() -> std::ffi::OsString {
    quiet_cargo().unwrap_or_else(|| std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn quiet_cargo() -> Option<std::ffi::OsString> {
    #[cfg(windows)]
    let path = std::path::PathBuf::from(std::env::var_os("USERPROFILE")?)
        .join(".claude/tools/quiet-cargo/quiet-cargo.cmd");
    #[cfg(not(windows))]
    let path = std::path::PathBuf::from(std::env::var_os("HOME")?)
        .join(".claude/tools/quiet-cargo/quiet-cargo");
    path.is_file().then(|| path.into_os_string())
}
