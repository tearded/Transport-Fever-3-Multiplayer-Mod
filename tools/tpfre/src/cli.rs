//! The command line: `tpfre index`, `tpfre q`, `tpfre diff`, `tpfre match`.

use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;

use crate::query::Query;
use crate::{archive, audit, build_gate, diff, index, matching, sig, update};

#[derive(Parser, Debug)]
#[command(
    name = "tpfre",
    version,
    about = "Index a game executable into one SQLite file, then ask it small questions.",
    after_help = "Addresses are RVAs in 0x hex (a VA inside the image is accepted too). \
                  A name argument is an exact name from any source, or a case-insensitive regex."
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Snapshot executable, root libraries and script/API sources privately.
    Archive {
        #[arg(long)]
        game: PathBuf,
        /// A new directory outside Git and the game install; never overwritten.
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        build: String,
        #[arg(long, default_value = "TransportFever3.exe")]
        exe: String,
        #[arg(long)]
        steam_manifest: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Compare every old profile target and its function against a new build.
    Audit {
        /// Complete archive directory or explicit executable (no script diff).
        old: PathBuf,
        new: PathBuf,
        #[arg(long)]
        profiles: PathBuf,
        /// SHA-keyed private index cache [default: OS temp/tpfre-audit].
        #[arg(long)]
        cache: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Strict identity, signature and prologue check of every profile target.
    Verify {
        binary: PathBuf,
        #[arg(long)]
        profiles: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Check the selected compiled bundle against a complete private archive.
    VerifyBuild {
        #[arg(long)]
        archive: PathBuf,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Verify the selected game build, then build the release binaries locally.
    Build {
        #[arg(long)]
        archive: PathBuf,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[arg(long)]
        jobs: Option<std::num::NonZeroUsize>,
    },
    /// Run the update pipeline: snapshot, compare, verify, test and build.
    Update {
        /// Previous complete private archive (the supported baseline).
        #[arg(long)]
        old: PathBuf,
        /// An existing complete archive of the updated game.
        #[arg(long, required_unless_present = "game", conflicts_with = "game")]
        new: Option<PathBuf>,
        /// Snapshot an installed game instead of supplying --new.
        #[arg(
            long,
            required_unless_present = "new",
            conflicts_with = "new",
            requires = "build"
        )]
        game: Option<PathBuf>,
        #[arg(long, requires = "game")]
        build: Option<String>,
        #[arg(long, default_value = "TransportFever3.exe")]
        exe: String,
        #[arg(long, requires = "game")]
        steam_manifest: Option<PathBuf>,
        /// New private run directory, outside Git and all game/archive inputs.
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[arg(long)]
        cache: Option<PathBuf>,
        #[arg(long)]
        jobs: Option<std::num::NonZeroUsize>,
        /// Stop after the static checks, without running Cargo.
        #[arg(long)]
        check_only: bool,
    },
    /// Index a PE32+ x86-64 binary (read only) into a .tpfdb file.
    Index {
        /// The executable, or a raw memory dump of it (see --image-base).
        binary: PathBuf,
        /// Output database [default: <binary stem>.tpfdb in the current directory].
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// The binary is a memory dump of the image loaded at this address
        /// (hex): sections are read at their RVAs, pointers against this base.
        #[arg(long, value_parser = parse_u64)]
        image_base: Option<u64>,
        /// Worker threads [default: all cores].
        #[arg(long)]
        threads: Option<usize>,
    },
    /// Query a .tpfdb file.
    Q {
        db: PathBuf,
        /// The binary to read bytes from and check against the database's
        /// SHA-256 [default: the path recorded at indexing].
        #[arg(long, global = true)]
        bin: Option<PathBuf>,
        /// Print a JSON array of facts instead of lines.
        #[arg(long, global = true)]
        json: bool,
        #[command(subcommand)]
        cmd: QCmd,
    },
    /// Carry the old build's function names to a new build that lacks them
    /// (TF3 has no __FUNCSIG__ strings), matched by shared strings, RTTI
    /// slots and the call graph; written into NEW as source 'matched'.
    Match {
        old: PathBuf,
        new: PathBuf,
        /// Report only; write nothing.
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value_t = 200)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Named functions moved, resized, appeared or disappeared between builds.
    Diff {
        old: PathBuf,
        new: PathBuf,
        /// Also list functions that only moved.
        #[arg(long)]
        moved: bool,
        #[arg(long, default_value_t = 500)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum QCmd {
    /// Binary identity, sections, counts and warnings (packers, SteamStub).
    Info,
    /// A function: bounds, every name with its source, source file, callers,
    /// callees, strings, vtable slots.
    Func {
        x: String,
        /// Functions to show when a regex matches several.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Lines per list (callees, strings, ...).
        #[arg(long, default_value_t = 40)]
        lines: usize,
    },
    /// Annotated disassembly (targets and RIP-relative operands named).
    Dis {
        x: String,
        /// Instructions from a function start.
        #[arg(long, default_value_t = 400)]
        max: usize,
        /// Instructions around an address inside a function.
        #[arg(long, default_value_t = 20)]
        context: usize,
        /// Show instruction bytes.
        #[arg(long)]
        bytes: bool,
    },
    /// Direct callers (tail jumps included), breadth first.
    Callers {
        x: String,
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long, default_value_t = 200)]
        limit: usize,
    },
    /// Direct callees (tail jumps and imports included), breadth first.
    Callees {
        x: String,
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long, default_value_t = 200)]
        limit: usize,
    },
    /// Shortest direct-call path from A to B.
    Path {
        a: String,
        b: String,
        #[arg(long, default_value_t = 12)]
        max_depth: usize,
    },
    /// Everything that references an address: calls, RIP-relative operands,
    /// data pointers, vtable slots.
    Xrefs {
        x: String,
        #[arg(long, default_value_t = 200)]
        limit: usize,
    },
    /// Strings matching a regex (case-insensitive unless --case), with the
    /// functions that reference them.
    Str {
        regex: String,
        #[arg(long)]
        case: bool,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Strings a function references.
    Fnstr { x: String },
    /// Names matching a regex, from every source.
    Names {
        regex: String,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Vtables of classes matching a regex, with their slots.
    Class {
        regex: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = 64)]
        lines: usize,
    },
    /// The vtable at (or holding) an address.
    Vtable {
        x: String,
        #[arg(long, default_value_t = 256)]
        lines: usize,
    },
    /// Functions attributed to source files containing a substring.
    File {
        substr: String,
        #[arg(long, default_value_t = 500)]
        limit: usize,
    },
    /// Every naming source for an address.
    Whois { x: String },
    /// A unique byte signature for a function start (make_profile's rules).
    Sig {
        x: String,
        /// Minimum prologue bytes (14 far jump; 5 near).
        #[arg(long, default_value_t = sig::FAR_STEAL)]
        steal: usize,
        #[arg(long, default_value_t = sig::DEFAULT_MAX_LENGTH)]
        max_length: usize,
        /// Do not require a stealable prologue (signature only).
        #[arg(long)]
        no_prologue: bool,
        /// Print a hook profile [[target]] block.
        #[arg(long)]
        toml: bool,
    },
    /// Check known RVAs against the naming (name_functions.py --validate).
    Validate { spec: PathBuf },
    /// Search code for a byte pattern (`48 8B ?? 05`).
    Bytes {
        pattern: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

fn parse_u64(s: &str) -> Result<u64, String> {
    let h = s.trim_start_matches("0x").trim_start_matches("0X");
    u64::from_str_radix(h, 16).map_err(|e| format!("{s:?} is not a hex address: {e}"))
}

/// Run a command line; returns the exit status.
pub fn run<I, T>(args: I, out: &mut dyn Write, err: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            let text = e.render().to_string();
            if e.use_stderr() {
                let _ = write!(err, "{text}");
            } else {
                let _ = write!(out, "{text}");
            }
            return code;
        }
    };
    match dispatch(cli, out, err) {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(err, "tpfre: {e:#}");
            3
        }
    }
}

fn dispatch(cli: Cli, out: &mut dyn Write, err: &mut dyn Write) -> anyhow::Result<i32> {
    match cli.cmd {
        Cmd::Archive {
            game,
            out: dest,
            build,
            exe,
            steam_manifest,
            json,
        } => {
            let m = archive::create(&archive::Options {
                game: &game,
                out: &dest,
                build: &build,
                executable: &exe,
                steam_manifest: steam_manifest.as_deref(),
            })?;
            if json {
                serde_json::to_writer_pretty(&mut *out, &m)?;
                writeln!(out)?;
            } else {
                writeln!(
                    out,
                    "archived build {}: {} files from {} sources -> {}",
                    m.build,
                    m.files.len(),
                    m.sources.len(),
                    dest.display()
                )?;
            }
            Ok(0)
        }
        Cmd::Audit {
            old,
            new,
            profiles,
            cache,
            json,
        } => {
            let cache = cache.unwrap_or_else(|| std::env::temp_dir().join("tpfre-audit"));
            audit::print(&audit::compare(&old, &new, &profiles, &cache)?, json, out)
        }
        Cmd::Verify {
            binary,
            profiles,
            json,
        } => audit::print(&audit::verify(&binary, &profiles)?, json, out),
        Cmd::VerifyBuild {
            archive,
            repo,
            json,
        } => build_gate::print(&build_gate::verify(&archive, &repo)?, json, out),
        Cmd::Build {
            archive,
            repo,
            jobs,
        } => build_gate::build(&archive, &repo, jobs, out),
        Cmd::Update {
            old,
            new,
            game,
            build,
            exe,
            steam_manifest,
            out: dest,
            repo,
            cache,
            jobs,
            check_only,
        } => {
            let input = match (new.as_deref(), game.as_deref(), build.as_deref()) {
                (Some(snapshot), None, None) => update::Input::Archive(snapshot),
                (None, Some(game), Some(build)) => update::Input::Install {
                    game,
                    build,
                    executable: &exe,
                    steam_manifest: steam_manifest.as_deref(),
                },
                _ => anyhow::bail!("use --new <archive> or --game <install> --build <ID>"),
            };
            update::run(
                &update::Options {
                    old: &old,
                    input,
                    repository: &repo,
                    output: &dest,
                    cache: cache.as_deref(),
                    jobs,
                    check_only,
                },
                out,
            )
        }
        Cmd::Index {
            binary,
            out: db,
            image_base,
            threads,
        } => {
            if let Some(n) = threads {
                let _ = rayon::ThreadPoolBuilder::new()
                    .num_threads(n)
                    .build_global();
            }
            let db = db.unwrap_or_else(|| {
                let stem = binary
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "binary".into());
                PathBuf::from(format!("{stem}.tpfdb"))
            });
            let r = index::run(&index::Options {
                binary: binary.clone(),
                out: db.clone(),
                dump_base: image_base,
            })?;
            writeln!(out, "indexed {} -> {}", binary.display(), db.display())?;
            for (k, v) in &r.stats {
                writeln!(out, "{k} {v}")?;
            }
            for w in &r.warnings {
                writeln!(err, "warning {w}")?;
            }
            writeln!(out, "seconds {:.2} db_bytes {}", r.seconds, r.db_bytes)?;
            Ok(0)
        }
        Cmd::Diff {
            old,
            new,
            moved,
            limit,
            json,
        } => diff::run(&old, &new, moved, limit, json, out),
        Cmd::Match {
            old,
            new,
            dry_run,
            limit,
            json,
        } => matching::run(&old, &new, dry_run, limit, json, out),
        Cmd::Q { db, bin, json, cmd } => {
            let mut q = Query::open(&db, bin, json, out)?;
            let code = match cmd {
                QCmd::Info => q.info()?,
                QCmd::Func { x, limit, lines } => q.func(&x, limit, lines)?,
                QCmd::Dis {
                    x,
                    max,
                    context,
                    bytes,
                } => q.dis(&x, max, context, bytes)?,
                QCmd::Callers { x, depth, limit } => q.graph(&x, true, depth, limit)?,
                QCmd::Callees { x, depth, limit } => q.graph(&x, false, depth, limit)?,
                QCmd::Path { a, b, max_depth } => q.path(&a, &b, max_depth)?,
                QCmd::Xrefs { x, limit } => q.xrefs(&x, limit)?,
                QCmd::Str { regex, case, limit } => q.strings(&regex, case, limit)?,
                QCmd::Fnstr { x } => q.fnstr(&x)?,
                QCmd::Names { regex, limit } => q.names(&regex, limit)?,
                QCmd::Class {
                    regex,
                    limit,
                    lines,
                } => q.class(&regex, limit, lines)?,
                QCmd::Vtable { x, lines } => q.vtable(&x, lines)?,
                QCmd::File { substr, limit } => q.file(&substr, limit)?,
                QCmd::Whois { x } => q.whois(&x)?,
                QCmd::Sig {
                    x,
                    steal,
                    max_length,
                    no_prologue,
                    toml,
                } => q.sig(
                    &x,
                    &sig::Options {
                        steal,
                        max_length,
                        prologue: !no_prologue,
                    },
                    toml,
                )?,
                QCmd::Bytes { pattern, limit } => q.bytes(&pattern, limit)?,
                QCmd::Validate { spec } => q.validate(&spec)?,
            };
            q.finish()?;
            Ok(code)
        }
    }
}
