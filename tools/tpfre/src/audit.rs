//! Per-target patch reports using the hook's actual profile parser and scanner.
//! A match is evidence, never permission to install a hook or publish a build.

use crate::{archive, db, index, matching, pe::Pe};
use anyhow::{Context, Result, ensure};
use iced_x86::{Decoder, DecoderOptions, OpKind};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::Path,
};
use tpf3mp_hookcore::profile::{BuildIdentity, Profile, TargetSpec};

#[derive(Debug, Serialize)]
pub struct Identity {
    pub sha256: String,
    pub size: u64,
    pub pe_timestamp: Option<u32>,
}
impl From<BuildIdentity> for Identity {
    fn from(v: BuildIdentity) -> Self {
        Self {
            sha256: v.sha256,
            size: v.size.unwrap_or(0),
            pe_timestamp: v.pe_timestamp,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TargetReport {
    pub profile: String,
    pub name: String,
    pub required: bool,
    pub status: String,
    pub hits: usize,
    pub old_rva: Option<u64>,
    pub new_rva: Option<u64>,
    pub expected_bytes: String,
    pub actual_bytes: Option<String>,
    pub old_function: Option<u32>,
    pub new_function: Option<u32>,
    pub normalized_function_equal: Option<bool>,
    pub comparison_error: Option<String>,
    /// A matcher hint for manual investigation, never a replacement target.
    pub suggested_function: Option<u32>,
    pub match_evidence: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ScriptDiff {
    pub available: bool,
    pub reason: Option<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub format_version: u32,
    pub kind: &'static str,
    pub old: Option<Identity>,
    pub new: Identity,
    pub targets: Vec<TargetReport>,
    pub scripts: ScriptDiff,
    pub warnings: Vec<String>,
    pub review_required: bool,
    pub runtime_verified: bool,
}

struct Scan {
    status: &'static str,
    hits: usize,
    rva: Option<u64>,
    actual: Option<String>,
}
fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
fn section<'a>(pe: &'a Pe<'a>, profile: &Profile) -> Result<(&'a [u8], u64)> {
    let name = profile.region.as_deref().unwrap_or(".text");
    let sec = pe
        .sections
        .iter()
        .find(|s| s.name == name && s.exec())
        .context("profile region is not an executable section")?;
    let raw = pe
        .data
        .get(sec.raw_off..sec.raw_off + sec.raw_size)
        .context("truncated profile section")?;
    Ok((raw, u64::from(sec.rva)))
}
fn scan(t: &TargetSpec, bytes: &[u8], base: u64) -> Scan {
    let hits = t.pattern().find_all(bytes);
    let status = match hits.len() {
        0 => "missing",
        1 => "matched",
        _ => "ambiguous",
    };
    if hits.len() != 1 {
        return Scan {
            status,
            hits: hits.len(),
            rva: None,
            actual: None,
        };
    }
    let at = i128::from(hits[0] as u64) + i128::from(t.offset);
    let Ok(at) = usize::try_from(at) else {
        return Scan {
            status: "out_of_bounds",
            hits: 1,
            rva: None,
            actual: None,
        };
    };
    let actual = at
        .checked_add(t.prologue.len())
        .and_then(|end| bytes.get(at..end));
    Scan {
        status: match actual {
            None => "out_of_bounds",
            Some(b) if b != t.prologue => "prologue_changed",
            _ => "matched",
        },
        hits: 1,
        rva: actual.map(|_| base + at as u64),
        actual: actual.map(hex),
    }
}
fn profiles(dir: &Path, id: &BuildIdentity) -> Result<Vec<Profile>> {
    let paths = tpf3mp_hookcore::profile::profile_files(dir)?;
    let mut out = Vec::new();
    for path in paths {
        let p = Profile::from_toml(&fs::read_to_string(&path)?)
            .with_context(|| format!("profile {}", path.display()))?;
        if p.verify_identity(id).is_ok() {
            out.push(p);
        }
    }
    ensure!(
        !out.is_empty(),
        "no profile matches executable {} (nothing skipped)",
        id.sha256
    );
    Ok(out)
}
fn cached_index(path: &Path, sha: &str, cache: &Path) -> Result<Connection> {
    fs::create_dir_all(cache)?;
    let dest = cache.join(format!("{sha}.tpfdb"));
    if !dest.exists() {
        let tmp = tempfile::tempdir_in(cache)?;
        let indexed = tmp.path().join("index.tpfdb");
        index::run(&index::Options {
            binary: path.into(),
            out: indexed.clone(),
            dump_base: None,
        })?;
        // A racing writer can have finished the same index. Do not replace it.
        if let Err(e) = fs::hard_link(&indexed, &dest)
            && e.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(e.into());
        }
    }
    let conn = db::open_ro(&dest)?;
    ensure!(
        db::meta_req(&conn, "sha256")? == sha,
        "index belongs to another binary"
    );
    Ok(conn)
}
fn parent(conn: &Connection, at: u64) -> Result<Option<u32>> {
    Ok(conn
        .query_row(
            "SELECT func FROM chunks WHERE start<=?1 AND end>?1 ORDER BY start DESC LIMIT 1",
            [i64::try_from(at)?],
            |r| r.get(0),
        )
        .optional()?)
}
type Chunks = HashMap<u32, Vec<(u32, u32)>>;
fn chunks(conn: &Connection) -> Result<Chunks> {
    let mut out = HashMap::new();
    let mut st = conn.prepare("SELECT func,start,end FROM chunks ORDER BY start")?;
    for row in st.query_map([], |r| {
        Ok((
            r.get::<_, u32>(0)?,
            r.get::<_, u32>(1)?,
            r.get::<_, u32>(2)?,
        ))
    })? {
        let (f, s, e) = row?;
        out.entry(f).or_insert_with(Vec::new).push((s, e));
    }
    Ok(out)
}
/// Excludes address bytes only. Retains field displacements and other constants.
/// Equality deliberately makes no claim about the referenced data or callee ABI.
fn normalized(pe: &Pe, chunks: &Chunks, func: u32) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    for &(start, end) in chunks.get(&func).context("function has no chunks")? {
        let code = pe
            .read(start, (end - start) as usize)
            .context("function chunk outside image")?;
        let mut d = Decoder::with_ip(64, code, u64::from(start), DecoderOptions::NONE);
        while d.can_decode() {
            let ins = d.decode();
            ensure!(
                !ins.is_invalid(),
                "invalid instruction in compared function {func:#x}"
            );
            let at = (ins.ip() - u64::from(start)) as usize;
            let mut b = code[at..at + ins.len()].to_vec();
            let c = d.get_constant_offsets(&ins);
            if ins.is_ip_rel_memory_operand() {
                b[c.displacement_offset()..c.displacement_offset() + c.displacement_size()].fill(0);
            }
            if (0..ins.op_count()).any(|i| {
                matches!(
                    ins.op_kind(i),
                    OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64
                )
            }) {
                b[c.immediate_offset()..c.immediate_offset() + c.immediate_size()].fill(0);
            }
            for i in 0..ins.op_count() {
                if matches!(
                    ins.op_kind(i),
                    OpKind::Immediate32 | OpKind::Immediate64 | OpKind::Immediate32to64
                ) {
                    let v = ins.immediate(i);
                    if v >= pe.image_base && v < pe.image_base + u64::from(pe.size_of_image) {
                        b[c.immediate_offset()..c.immediate_offset() + c.immediate_size()].fill(0);
                    }
                }
            }
            out.push(b);
        }
    }
    Ok(out)
}
fn script_diff(old: Option<&Path>, new: &Path) -> Result<ScriptDiff> {
    let Some(old) = old.filter(|o| o.is_dir() && new.is_dir()) else {
        return Ok(ScriptDiff {available:false,reason:Some("Both inputs must be complete tpfre archives; EXE-only input cannot compare scripts".into()),added:vec![],removed:vec![],changed:vec![]});
    };
    let a = archive::verify(old)?;
    let b = archive::verify(new)?;
    let files = |m: &archive::Manifest| {
        m.files
            .iter()
            .filter(|f| {
                f.path.starts_with("scripts/")
                    || [".tl", ".lua", ".json", ".gs"]
                        .iter()
                        .any(|s| f.source.to_ascii_lowercase().ends_with(s))
            })
            .map(|f| (f.path.clone(), f.sha256.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let a = files(&a);
    let b = files(&b);
    Ok(ScriptDiff {
        available: true,
        reason: None,
        added: b.keys().filter(|k| !a.contains_key(*k)).cloned().collect(),
        removed: a.keys().filter(|k| !b.contains_key(*k)).cloned().collect(),
        changed: a
            .iter()
            .filter(|(k, v)| b.get(*k).is_some_and(|w| w != *v))
            .map(|(k, _)| k.clone())
            .collect(),
    })
}

pub fn compare(old: &Path, new: &Path, dir: &Path, cache: &Path) -> Result<Report> {
    let oldpath = archive::executable(old)?;
    let newpath = archive::executable(new)?;
    let oldbytes = fs::read(&oldpath)?;
    let newbytes = fs::read(&newpath)?;
    let oid = BuildIdentity::of_bytes(&oldbytes);
    let nid = BuildIdentity::of_bytes(&newbytes);
    let ps = profiles(dir, &oid)?;
    let mut op = Pe::parse(&oldbytes, None)?;
    op.compute_entropy();
    let mut np = Pe::parse(&newbytes, None)?;
    np.compute_entropy();
    let oc = cached_index(&oldpath, &oid.sha256, cache)?;
    let nc = cached_index(&newpath, &nid.sha256, cache)?;
    let ochunks = chunks(&oc)?;
    let nchunks = chunks(&nc)?;
    let pairs = matching::match_builds(&matching::load(&oc)?, &matching::load(&nc)?, &mut |_| {});
    let mapping: HashMap<_, _> = pairs.iter().map(|p| (p.old, p)).collect();
    let mut reports = Vec::new();
    for p in ps {
        let (ob, obase) = section(&op, &p)?;
        let (nb, nbase) = section(&np, &p)?;
        for t in &p.targets {
            let a = scan(t, ob, obase);
            ensure!(
                a.status == "matched",
                "baseline target {} is {}; audit cannot establish baseline",
                t.name,
                a.status
            );
            let b = scan(t, nb, nbase);
            let of = a.rva.map(|at| parent(&oc, at)).transpose()?.flatten();
            let nf = b.rva.map(|at| parent(&nc, at)).transpose()?.flatten();
            let pair = of.and_then(|f| mapping.get(&f).copied());
            let comparison = match (of, nf) {
                (Some(a), Some(b)) => normalized(&op, &ochunks, a)
                    .and_then(|old| normalized(&np, &nchunks, b).map(|new| old == new)),
                _ => Err(anyhow::anyhow!(
                    "containing function unavailable for old or new target"
                )),
            };
            let (equal, comparison_error) = match comparison {
                Ok(equal) => (Some(equal), None),
                Err(e) => (None, Some(format!("{e:#}"))),
            };
            reports.push(TargetReport {
                profile: p.name.clone(),
                name: t.name.clone(),
                required: t.required,
                status: b.status.into(),
                hits: b.hits,
                old_rva: a.rva,
                new_rva: b.rva,
                expected_bytes: hex(&t.prologue),
                actual_bytes: b.actual,
                old_function: of,
                new_function: nf,
                normalized_function_equal: equal,
                comparison_error,
                suggested_function: pair.map(|p| p.new),
                match_evidence: pair.map(|p| format!("{}: {}", p.how, p.why)),
            });
        }
    }
    // Refuse a race with Steam even when an earlier cached index was reused.
    ensure!(
        BuildIdentity::of_file(&oldpath)?.sha256 == oid.sha256
            && BuildIdentity::of_file(&newpath)?.sha256 == nid.sha256,
        "executable changed during audit"
    );
    let scripts = script_diff(Some(old), new)?;
    let review = reports
        .iter()
        .any(|t| t.status != "matched" || t.normalized_function_equal != Some(true))
        || !scripts.available
        || !scripts.changed.is_empty()
        || !scripts.added.is_empty()
        || !scripts.removed.is_empty();
    let mut warnings=vec!["Normalized function equality excludes address operands; it does not prove callee/data equivalence, ABI, or runtime compatibility. Matcher suggestions require manual review.".into()];
    for (label, pe) in [("old", &op), ("new", &np)] {
        if pe.sections.iter().any(|s| s.packed() && s.name != ".bind") {
            warnings.push(format!("{label}: executable section appears packed; inspect protection before using static findings"));
        }
    }
    let review = review || warnings.len() > 1;
    Ok(Report {
        format_version: 1,
        kind: "patch_audit",
        old: Some(oid.into()),
        new: nid.into(),
        targets: reports,
        scripts,
        warnings,
        review_required: review,
        runtime_verified: false,
    })
}

/// All targets, including optional ones, must resolve. No absent-EXE/build skips.
pub fn verify(input: &Path, dir: &Path) -> Result<Report> {
    let exe = archive::executable(input)?;
    let bytes = fs::read(&exe)?;
    let id = BuildIdentity::of_bytes(&bytes);
    let ps = profiles(dir, &id)?;
    let pe = Pe::parse(&bytes, None)?;
    let mut targets = Vec::new();
    for p in ps {
        let (b, base) = section(&pe, &p)?;
        for t in &p.targets {
            let s = scan(t, b, base);
            targets.push(TargetReport {
                profile: p.name.clone(),
                name: t.name.clone(),
                required: t.required,
                status: s.status.into(),
                hits: s.hits,
                old_rva: None,
                new_rva: s.rva,
                expected_bytes: hex(&t.prologue),
                actual_bytes: s.actual,
                old_function: None,
                new_function: None,
                normalized_function_equal: None,
                comparison_error: None,
                suggested_function: None,
                match_evidence: None,
            });
        }
    }
    ensure!(
        BuildIdentity::of_file(&exe)?.sha256 == id.sha256,
        "executable changed during verification"
    );
    let review = targets.iter().any(|t| t.status != "matched");
    Ok(Report {
        format_version: 1,
        kind: "profile_verification",
        old: None,
        new: id.into(),
        targets,
        scripts: script_diff(None, input)?,
        warnings: vec!["This verifies profile bytes only, not ABI or runtime acceptance.".into()],
        review_required: review,
        runtime_verified: false,
    })
}

pub fn print(report: &Report, json: bool, out: &mut dyn Write) -> Result<i32> {
    if json {
        serde_json::to_writer_pretty(&mut *out, report)?;
        writeln!(out)?;
    } else {
        writeln!(out, "{} {}", report.kind, report.new.sha256)?;
        let matched = report
            .targets
            .iter()
            .filter(|t| t.status == "matched")
            .count();
        let changed = report
            .targets
            .iter()
            .filter(|t| t.normalized_function_equal == Some(false))
            .count();
        writeln!(
            out,
            "targets {} matched {matched} function_changed {changed}",
            report.targets.len()
        )?;
        for t in &report.targets {
            if t.status != "matched"
                || (report.kind == "patch_audit" && t.normalized_function_equal != Some(true))
            {
                writeln!(
                    out,
                    "{} {} old={:?} new={:?} function_equal={:?} suggested_function={:?}",
                    t.status,
                    t.name,
                    t.old_rva.map(|v| format!("0x{v:x}")),
                    t.new_rva.map(|v| format!("0x{v:x}")),
                    t.normalized_function_equal,
                    t.suggested_function.map(|v| format!("0x{v:x}"))
                )?;
                if let Some(e) = &t.comparison_error {
                    writeln!(out, "comparison_unavailable {e}")?;
                }
            }
        }
        writeln!(
            out,
            "scripts available={} added={} removed={} changed={}",
            report.scripts.available,
            report.scripts.added.len(),
            report.scripts.removed.len(),
            report.scripts.changed.len()
        )?;
        if let Some(reason) = &report.scripts.reason {
            writeln!(out, "warning {reason}")?;
        }
        for w in &report.warnings {
            writeln!(out, "warning {w}")?;
        }
        writeln!(
            out,
            "review_required={} runtime_verified=false",
            report.review_required
        )?;
    }
    Ok(i32::from(report.review_required))
}
