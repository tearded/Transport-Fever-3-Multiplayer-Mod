//! Private, immutable snapshots of executables and the game's script/API sources.
//! Reads the install only. A failed snapshot retains an `.incomplete` marker.

use anyhow::{Context, Result, ensure};
use flate2::read::DeflateDecoder;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};
use tpf3mp_hookcore::profile::BuildIdentity;

const VERSION: u32 = 1;
const MAX_SCRIPT: u64 = 64 << 20;
const MAX_TOTAL: u64 = 1 << 30;

#[derive(Debug, Serialize, Deserialize)]
pub struct Source {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// `file` for loose inputs; `script_entries` for the selected ZIP contents.
    pub hash_scope: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArchivedFile {
    pub path: String,
    pub source: String,
    pub entry: Option<String>,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub tool_version: String,
    pub build: String,
    pub executable: String,
    pub sources: Vec<Source>,
    pub files: Vec<ArchivedFile>,
}

pub struct Options<'a> {
    pub game: &'a Path,
    pub out: &'a Path,
    pub build: &'a str,
    pub executable: &'a str,
    pub steam_manifest: Option<&'a Path>,
}

fn source_text(path: &str) -> bool {
    [".tl", ".lua", ".json", ".gs"]
        .iter()
        .any(|s| path.to_ascii_lowercase().ends_with(s))
}

fn safe_path(name: &str) -> Result<PathBuf> {
    ensure!(
        !name.is_empty() && !name.contains(['\\', ':', '\0']),
        "unsafe archive path {name:?}"
    );
    let path = PathBuf::from(name);
    ensure!(
        path.components().all(|c| matches!(c, Component::Normal(_))),
        "unsafe archive path {name:?}"
    );
    // ZIP names use '/', independently of the host OS. Refuse traversal on Linux too.
    ensure!(
        name.split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".."),
        "unsafe archive path {name:?}"
    );
    Ok(path)
}

fn files_under(root: &Path) -> Result<Vec<PathBuf>> {
    let mut todo = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = todo.pop() {
        for e in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let e = e?;
            let kind = e.file_type()?;
            ensure!(
                !kind.is_symlink(),
                "refusing linked input {}",
                e.path().display()
            );
            if kind.is_dir() {
                todo.push(e.path());
            } else if kind.is_file() {
                files.push(e.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn inventory(files: &[PathBuf]) -> Result<Vec<(PathBuf, u64, std::time::SystemTime)>> {
    files
        .iter()
        .map(|path| {
            let m = fs::metadata(path)?;
            Ok((path.clone(), m.len(), m.modified()?))
        })
        .collect()
}

fn entries_digest(entries: &[(String, u64, String)]) -> Result<String> {
    Ok(crate::index::sha256_hex(&serde_json::to_vec(entries)?))
}

fn zip_digest(path: &Path) -> Result<String> {
    let mut f = File::open(path)?;
    let mut records = Vec::new();
    let mut total = 0u64;
    for e in script_entries(&mut f)? {
        total = total
            .checked_add(u64::from(e.size))
            .context("script budget overflow")?;
        ensure!(total <= MAX_TOTAL, "script verification exceeds budget");
        records.push((
            e.name.clone(),
            u64::from(e.size),
            crate::index::sha256_hex(&read_script(&mut f, &e)?),
        ));
    }
    entries_digest(&records)
}

fn label(path: &Path, root: &Path) -> Result<String> {
    let s = path
        .strip_prefix(root)?
        .to_str()
        .context("non-UTF8 source path")?
        .replace('\\', "/");
    safe_path(&s)?;
    Ok(s)
}

fn write_new(root: &Path, name: &str, bytes: &[u8]) -> Result<ArchivedFile> {
    let path = root.join(safe_path(name)?);
    fs::create_dir_all(path.parent().context("no parent")?)?;
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    f.write_all(bytes)?;
    // The manifest is the completion boundary. Consumers rehash every file;
    // per-script disk flushes would make thousands of tiny sources very slow.
    if name == "build.json" {
        f.sync_all()?;
    }
    Ok(ArchivedFile {
        path: name.into(),
        source: String::new(),
        entry: None,
        size: bytes.len() as u64,
        sha256: crate::index::sha256_hex(bytes),
    })
}

fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        b.get(at..at + 2)
            .context("truncated ZIP field")?
            .try_into()?,
    ))
}
fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        b.get(at..at + 4)
            .context("truncated ZIP field")?
            .try_into()?,
    ))
}

struct ZipEntry {
    name: String,
    method: u16,
    compressed: u32,
    size: u32,
    crc: u32,
    local: u32,
}

/// The central directory is standard ZIP; TF3 uses UG instead of PK locally.
fn script_entries(f: &mut File) -> Result<Vec<ZipEntry>> {
    let len = f.metadata()?.len();
    let tail_len = len.min(65_557) as usize;
    f.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = vec![0; tail_len];
    f.read_exact(&mut tail)?;
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| {
            tail.get(i..i + 4) == Some(b"PK\x05\x06")
                && u16_at(&tail, i + 20).is_ok_and(|n| i + 22 + usize::from(n) == tail.len())
        })
        .context("no ZIP end of central directory")?;
    ensure!(
        u16_at(&tail, end + 4)? == 0 && u16_at(&tail, end + 6)? == 0,
        "multi-disk ZIP unsupported"
    );
    let count = u16_at(&tail, end + 10)?;
    ensure!(
        count != u16::MAX && u16_at(&tail, end + 8)? == count,
        "ZIP64 or split ZIP unsupported"
    );
    let size = u32_at(&tail, end + 12)?;
    let start = u32_at(&tail, end + 16)?;
    ensure!(
        u64::from(start) + u64::from(size) <= len - tail_len as u64 + end as u64,
        "bad ZIP directory bounds"
    );
    ensure!(size <= 64 << 20, "ZIP directory exceeds budget");
    f.seek(SeekFrom::Start(u64::from(start)))?;
    let mut dir = vec![0; size as usize];
    f.read_exact(&mut dir)?;
    let mut at = 0;
    let mut out = Vec::new();
    let mut names = BTreeSet::new();
    for _ in 0..count {
        ensure!(
            dir.get(at..at + 4) == Some(b"PK\x01\x02"),
            "bad ZIP directory entry"
        );
        let flags = u16_at(&dir, at + 8)?;
        let method = u16_at(&dir, at + 10)?;
        let crc = u32_at(&dir, at + 16)?;
        let compressed = u32_at(&dir, at + 20)?;
        let size = u32_at(&dir, at + 24)?;
        let n = usize::from(u16_at(&dir, at + 28)?);
        let extra = usize::from(u16_at(&dir, at + 30)?);
        let comment = usize::from(u16_at(&dir, at + 32)?);
        let local = u32_at(&dir, at + 42)?;
        let name = std::str::from_utf8(
            dir.get(at + 46..at + 46 + n)
                .context("truncated ZIP name")?,
        )?
        .to_owned();
        at = at
            .checked_add(46 + n + extra + comment)
            .context("ZIP directory overflow")?;
        ensure!(at <= dir.len(), "truncated ZIP entry");
        if !source_text(&name) {
            continue;
        }
        safe_path(&name)?;
        ensure!(names.insert(name.clone()), "duplicate ZIP script {name}");
        ensure!(
            flags & 1 == 0 && matches!(method, 0 | 8),
            "encrypted/unsupported ZIP script {name}"
        );
        ensure!(
            u64::from(size) <= MAX_SCRIPT && u64::from(compressed) <= MAX_SCRIPT,
            "ZIP script exceeds budget: {name}"
        );
        ensure!(local != u32::MAX, "ZIP64 script unsupported: {name}");
        out.push(ZipEntry {
            name,
            method,
            compressed,
            size,
            crc,
            local,
        });
    }
    ensure!(at == dir.len(), "unparsed ZIP directory bytes");
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn read_script(f: &mut File, e: &ZipEntry) -> Result<Vec<u8>> {
    f.seek(SeekFrom::Start(u64::from(e.local)))?;
    let mut h = [0; 30];
    f.read_exact(&mut h)?;
    ensure!(
        matches!(&h[..4], b"PK\x03\x04" | b"UG\x03\x04"),
        "bad ZIP local header for {}",
        e.name
    );
    ensure!(
        u16_at(&h, 6)? & 1 == 0 && u16_at(&h, 8)? == e.method,
        "inconsistent ZIP local header"
    );
    let mut name = vec![0; usize::from(u16_at(&h, 26)?)];
    f.read_exact(&mut name)?;
    ensure!(
        name == e.name.as_bytes(),
        "ZIP local/directory name mismatch"
    );
    f.seek(SeekFrom::Current(i64::from(u16_at(&h, 28)?)))?;
    let mut compressed = vec![0; e.compressed as usize];
    f.read_exact(&mut compressed)?;
    let bytes = if e.method == 0 {
        compressed
    } else {
        let mut d = DeflateDecoder::new(compressed.as_slice());
        let mut out = Vec::new();
        Read::by_ref(&mut d)
            .take(u64::from(e.size) + 1)
            .read_to_end(&mut out)?;
        ensure!(
            d.total_in() == u64::from(e.compressed),
            "trailing ZIP compressed data"
        );
        out
    };
    ensure!(
        bytes.len() == e.size as usize && crc32fast::hash(&bytes) == e.crc,
        "bad ZIP size/CRC for {}",
        e.name
    );
    Ok(bytes)
}

fn check_output(game: &Path, out: &Path) -> Result<()> {
    ensure!(
        !out.components().any(|c| matches!(c, Component::ParentDir)),
        "output must not contain '..'"
    );
    let out = std::path::absolute(out)?;
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut existing = parent.to_path_buf();
    while !existing.exists() {
        ensure!(existing.pop(), "no existing output ancestor");
    }
    let ancestor = existing.canonicalize()?;
    ensure!(
        !ancestor.starts_with(game),
        "archive must be outside the game install"
    );
    ensure!(
        !ancestor.ancestors().any(|p| p.join(".git").exists()),
        "archive must be outside a Git worktree (private game sources)"
    );
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    ensure!(
        !parent.starts_with(game),
        "archive must be outside the game install"
    );
    ensure!(
        !out.exists(),
        "archive already exists; nothing overwritten: {}",
        out.display()
    );
    Ok(())
}

pub fn create(opts: &Options<'_>) -> Result<Manifest> {
    ensure!(
        !opts.build.is_empty()
            && opts
                .build
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "unsafe build label"
    );
    let game = opts.game.canonicalize()?;
    ensure!(game.is_dir(), "game is not a directory");
    let exe = safe_path(opts.executable)?;
    ensure!(
        exe.components().count() == 1,
        "executable must be a file in the game root"
    );
    ensure!(game.join(&exe).is_file(), "executable missing");
    check_output(&game, opts.out)?;
    let inferred = game
        .parent()
        .and_then(Path::parent)
        .map(|p| p.join("appmanifest_3493540.acf"));
    let steam = opts
        .steam_manifest
        .map(Path::to_path_buf)
        .or_else(|| inferred.filter(|p| p.is_file()));
    let steam_bytes = steam.as_ref().map(fs::read).transpose()?;
    if let Some(bytes) = &steam_bytes {
        let re = regex::Regex::new(r#""buildid"\s+"([0-9]+)""#)?;
        let text = std::str::from_utf8(bytes)?;
        let id = re
            .captures(text)
            .and_then(|c| c.get(1))
            .context("Steam manifest has no buildid")?;
        ensure!(
            id.as_str() == opts.build,
            "Steam build changed or --build is wrong: {}",
            id.as_str()
        );
    }
    let input_files = files_under(&game)?;
    let initial_inventory = inventory(&input_files)?;
    fs::create_dir(opts.out)?;
    fs::write(
        opts.out.join(".incomplete"),
        b"Snapshot incomplete: never use for verification\n",
    )?;
    let mut manifest = Manifest {
        format_version: VERSION,
        tool_version: env!("CARGO_PKG_VERSION").into(),
        build: opts.build.into(),
        executable: format!("files/{}", opts.executable),
        sources: Vec::new(),
        files: Vec::new(),
    };
    let mut total = 0u64;
    for path in &input_files {
        let rel = label(path, &game)?;
        let binary = path.parent() == Some(game.as_path())
            && (rel == opts.executable
                || [".exe", ".dll", ".so", ".dylib"]
                    .iter()
                    .any(|s| rel.to_ascii_lowercase().ends_with(s)));
        let loose = source_text(&rel);
        let zip = rel.to_ascii_lowercase().ends_with(".zip");
        if !binary && !loose && !zip {
            continue;
        }
        let mut f = File::open(path)?;
        let entries = if zip {
            script_entries(&mut f).with_context(|| format!("reading {}", path.display()))?
        } else {
            Vec::new()
        };
        if zip && entries.is_empty() {
            continue;
        }
        if !zip {
            let id = BuildIdentity::of_file(path)?;
            manifest.sources.push(Source {
                path: rel.clone(),
                size: id.size.context("no size")?,
                sha256: id.sha256,
                hash_scope: "file".into(),
            });
            ensure!(
                binary || f.metadata()?.len() <= MAX_SCRIPT,
                "loose script exceeds budget: {rel}"
            );
            if !binary {
                total = total
                    .checked_add(f.metadata()?.len())
                    .context("script budget overflow")?;
                ensure!(total <= MAX_TOTAL, "total scripts exceed budget");
            }
            let bytes = fs::read(path)?;
            let mut archived = write_new(opts.out, &format!("files/{rel}"), &bytes)?;
            ensure!(
                archived.sha256 == manifest.sources.last().context("no source")?.sha256,
                "input changed while copying {rel}"
            );
            archived.source = rel;
            manifest.files.push(archived);
        } else {
            let mut records = Vec::new();
            for entry in entries {
                total = total
                    .checked_add(u64::from(entry.size))
                    .context("script budget overflow")?;
                ensure!(total <= MAX_TOTAL, "total extracted scripts exceed budget");
                let bytes = read_script(&mut f, &entry)?;
                let mut archived =
                    write_new(opts.out, &format!("scripts/{rel}/{}", entry.name), &bytes)?;
                archived.source = rel.clone();
                records.push((
                    entry.name.clone(),
                    u64::from(entry.size),
                    archived.sha256.clone(),
                ));
                archived.entry = Some(entry.name);
                manifest.files.push(archived);
            }
            manifest.sources.push(Source {
                path: rel,
                size: f.metadata()?.len(),
                sha256: entries_digest(&records)?,
                hash_scope: "script_entries".into(),
            });
        }
    }
    if let Some(bytes) = steam_bytes {
        let mut entry = write_new(opts.out, "steam/appmanifest.acf", &bytes)?;
        entry.source = "Steam appmanifest".into();
        manifest.files.push(entry);
        ensure!(
            steam.as_ref().map(fs::read).transpose()?.as_deref() == Some(bytes.as_slice()),
            "Steam manifest changed during archive"
        );
    }
    ensure!(
        inventory(&files_under(&game)?)? == initial_inventory,
        "game file inventory changed during archive"
    );
    for source in &manifest.sources {
        let path = game.join(safe_path(&source.path)?);
        let hash = if source.hash_scope == "file" {
            BuildIdentity::of_file(&path)?.sha256
        } else {
            zip_digest(&path)?
        };
        ensure!(
            hash == source.sha256,
            "input changed during archive: {}",
            source.path
        );
    }
    manifest.files.sort_by(|a, b| a.path.cmp(&b.path));
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    write_new(opts.out, "build.json", &bytes)?;
    fs::remove_file(opts.out.join(".incomplete"))?;
    Ok(manifest)
}

/// Validate every recorded file, not merely the executable used by the audit.
pub fn verify(root: &Path) -> Result<Manifest> {
    ensure!(!root.join(".incomplete").exists(), "archive is incomplete");
    let m: Manifest = serde_json::from_slice(&fs::read(root.join("build.json"))?)
        .context("not a tpfre source archive (legacy EXE-only archives need explicit EXE input)")?;
    ensure!(
        m.format_version == VERSION,
        "unsupported archive format {}",
        m.format_version
    );
    let mut names = BTreeSet::new();
    for entry in &m.files {
        ensure!(names.insert(&entry.path), "duplicate archived path");
        let path = root.join(safe_path(&entry.path)?);
        let resolved = path.canonicalize()?;
        ensure!(
            resolved.starts_with(root.canonicalize()?),
            "archive file escapes root"
        );
        let id = BuildIdentity::of_file(&path)?;
        ensure!(
            id.sha256 == entry.sha256 && id.size == Some(entry.size),
            "archive hash/size mismatch: {}",
            entry.path
        );
    }
    ensure!(
        names.contains(&m.executable),
        "archive does not record its executable"
    );
    let mut sources = BTreeSet::new();
    for source in &m.sources {
        safe_path(&source.path)?;
        ensure!(sources.insert(&source.path), "duplicate archive source");
        let mut files: Vec<_> = m.files.iter().filter(|f| f.source == source.path).collect();
        ensure!(
            !files.is_empty(),
            "source has no archived files: {}",
            source.path
        );
        match source.hash_scope.as_str() {
            "file" => {
                ensure!(
                    files.len() == 1
                        && files[0].entry.is_none()
                        && files[0].sha256 == source.sha256
                        && files[0].size == source.size,
                    "loose source manifest mismatch: {}",
                    source.path
                );
            }
            "script_entries" => {
                files.sort_by(|a, b| a.entry.cmp(&b.entry));
                let records = files
                    .iter()
                    .map(|f| {
                        Ok((
                            f.entry.clone().context("ZIP source entry missing")?,
                            f.size,
                            f.sha256.clone(),
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                ensure!(
                    entries_digest(&records)? == source.sha256,
                    "ZIP source manifest mismatch: {}",
                    source.path
                );
            }
            _ => anyhow::bail!("unknown source hash scope {}", source.hash_scope),
        }
    }
    Ok(m)
}

pub fn executable(input: &Path) -> Result<PathBuf> {
    if input.is_dir() {
        let m = verify(input)?;
        Ok(input.join(safe_path(&m.executable)?))
    } else {
        ensure!(input.is_file(), "missing executable {}", input.display());
        Ok(input.to_path_buf())
    }
}
