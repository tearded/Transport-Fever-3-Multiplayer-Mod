//! `tpfre match`: carries function names from an old build to a new one
//! that lacks them.
//!
//! Transport Fever 3 keeps MSVC RTTI but drops the `__FUNCSIG__` strings
//! that named 20,000 of Transport Fever 2's functions, and `tpfre diff`
//! matches builds by exactly those names. This matches by what survives in
//! both builds instead:
//!
//! - **strings**: a string (an assert's condition, a log line, a Lua
//!   binding's name) that exactly one function uses in each build pairs
//!   those two functions; each such string is a vote;
//! - **RTTI**: the function filling slot N of class C's vtable in both
//!   builds, when the vtable has as many slots in each and no other slot
//!   holds it;
//! - **the call graph**, from those anchors outward: an unmatched callee
//!   (or caller) of a matched pair that has exactly one plausible partner
//!   among the other side's unmatched callees (or callers), and is that
//!   partner's only one too.
//!
//! A pair is kept only when each side is the other's single best
//! candidate. The result is written into the new database's `names` table
//! as source `matched`, so every query finds it; the old database is only
//! read. Nothing is matched by address.

use anyhow::{Result, bail};
use rusqlite::{Connection, params};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;

use crate::db;

/// Shorter strings are too common to identify a function ("%s", "true").
const MIN_STRING: usize = 8;
/// A function with more callers or callees than this (an allocator, the
/// assert handler) is a hub: its neighbours say nothing about which is
/// which, and comparing them all would take hours.
const MAX_NEIGHBOURS: usize = 64;

/// One build, as the matcher sees it.
#[derive(Debug, Default, Clone)]
pub struct Build {
    /// Instruction count per function.
    pub size: HashMap<u32, u32>,
    /// Callees per function, in call-site order, each once.
    pub callees: HashMap<u32, Vec<u32>>,
    /// Callers per function, each once.
    pub callers: HashMap<u32, Vec<u32>>,
    /// Plain strings (not `__FUNCSIG__` or `__FILE__`) and the functions
    /// that use each.
    pub strings: HashMap<String, Vec<u32>>,
    /// (class, vtable offset, slot, slots in the vtable) -> the function.
    pub slots: Vec<((String, u32, u32, u32), u32)>,
    /// The source file of a function the build names one for, as a path
    /// under the source tree, lowercase (`game/gamesim.cpp`).
    pub file: HashMap<u32, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub old: u32,
    pub new: u32,
    /// How it was found: `string(N)`, `rtti` or `calls`.
    pub how: String,
    /// The evidence, for a person checking it.
    pub why: String,
}

/// Matches `new`'s functions to `old`'s.
pub fn match_builds(old: &Build, new: &Build, progress: &mut dyn FnMut(String)) -> Vec<Pair> {
    let mut votes: HashMap<(u32, u32), (u32, String)> = HashMap::new();
    for (text, olds) in &old.strings {
        if text.len() < MIN_STRING || olds.len() != 1 {
            continue;
        }
        if let Some(news) = new.strings.get(text)
            && news.len() == 1
        {
            let e = votes
                .entry((olds[0], news[0]))
                .or_insert((0, String::new()));
            e.0 += 1;
            if e.1.is_empty() {
                e.1 = text.chars().take(80).collect();
            }
        }
    }
    let unique_slots = |b: &Build| {
        let mut count: HashMap<u32, u32> = HashMap::new();
        for (_, f) in &b.slots {
            *count.entry(*f).or_default() += 1;
        }
        b.slots
            .iter()
            .filter(|(_, f)| count[f] == 1)
            .map(|(k, f)| (k.clone(), *f))
            .collect::<HashMap<_, _>>()
    };
    let (old_slots, new_slots) = (unique_slots(old), unique_slots(new));
    let mut rtti: HashMap<(u32, u32), String> = HashMap::new();
    for (key, of) in &old_slots {
        if let Some(nf) = new_slots.get(key) {
            rtti.insert((*of, *nf), format!("{}::vf{}", key.0, key.2));
        }
    }

    // Anchors: the single best candidate on both sides.
    let mut scored: Vec<(u32, u32, u32, String, String)> = Vec::new();
    for (&(o, n), (count, text)) in &votes {
        scored.push((
            o,
            n,
            *count * 2,
            format!("string({count})"),
            format!("\"{text}\""),
        ));
    }
    for (&(o, n), slot) in &rtti {
        let bonus = votes.get(&(o, n)).map_or(0, |v| v.0 * 2);
        scored.push((o, n, 1 + bonus, "rtti".into(), slot.clone()));
    }
    let mut best_new: HashMap<u32, (u32, u32, bool)> = HashMap::new();
    let mut best_old: HashMap<u32, (u32, u32, bool)> = HashMap::new();
    for &(o, n, s, _, _) in &scored {
        for (map, key, other) in [(&mut best_new, n, o), (&mut best_old, o, n)] {
            match map.get_mut(&key) {
                None => {
                    map.insert(key, (other, s, false));
                }
                Some(b) if s > b.1 => *b = (other, s, false),
                Some(b) if s == b.1 && b.0 != other => b.2 = true,
                Some(_) => {}
            }
        }
    }
    let mut matched_old: HashMap<u32, u32> = HashMap::new();
    let mut matched_new: HashMap<u32, u32> = HashMap::new();
    let mut pairs: BTreeMap<u32, Pair> = BTreeMap::new();
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    for (o, n, _, how, why) in scored {
        if !seen.insert((o, n)) {
            continue;
        }
        let (Some(bn), Some(bo)) = (best_new.get(&n), best_old.get(&o)) else {
            continue;
        };
        if bn.0 != o || bo.0 != n || bn.2 || bo.2 || !same_file(old, new, o, n) {
            continue;
        }
        if matched_old.contains_key(&o) || matched_new.contains_key(&n) {
            continue;
        }
        let (how, why) = if rtti.contains_key(&(o, n)) && votes.contains_key(&(o, n)) {
            (format!("{how}+rtti"), why)
        } else {
            (how, why)
        };
        matched_old.insert(o, n);
        matched_new.insert(n, o);
        pairs.insert(
            n,
            Pair {
                old: o,
                new: n,
                how,
                why,
            },
        );
    }

    // Outward through the call graph, until nothing more is found.
    progress(format!(
        "anchors: {} pairs from strings and RTTI",
        pairs.len()
    ));
    // Each round looks only around the pairs the round before found.
    let empty: Vec<u32> = Vec::new();
    let mut frontier: Vec<(u32, u32)> = pairs.values().map(|p| (p.old, p.new)).collect();
    loop {
        let mut found: Vec<Pair> = Vec::new();
        let mut taken_old: HashSet<u32> = HashSet::new();
        let mut taken_new: HashSet<u32> = HashSet::new();
        for &(pair_old, n) in &frontier {
            let pair = Pair {
                old: pair_old,
                new: n,
                how: String::new(),
                why: String::new(),
            };
            for (edges_old, edges_new, what) in [
                (&old.callees, &new.callees, "callee"),
                (&old.callers, &new.callers, "caller"),
            ] {
                let all_old = edges_old.get(&pair.old).unwrap_or(&empty);
                let all_new = edges_new.get(&n).unwrap_or(&empty);
                if all_old.len() > MAX_NEIGHBOURS || all_new.len() > MAX_NEIGHBOURS {
                    continue;
                }
                let os: Vec<u32> = all_old
                    .iter()
                    .copied()
                    .filter(|f| !matched_old.contains_key(f) && !taken_old.contains(f))
                    .collect();
                let ns: Vec<u32> = all_new
                    .iter()
                    .copied()
                    .filter(|f| !matched_new.contains_key(f) && !taken_new.contains(f))
                    .collect();
                for &o in &os {
                    let fits = |a: u32, b: u32| plausible(old, new, a, b);
                    let cands: Vec<u32> = ns.iter().copied().filter(|&c| fits(o, c)).collect();
                    if cands.len() != 1 {
                        continue;
                    }
                    let c = cands[0];
                    let back = os.iter().filter(|&&x| fits(x, c)).count();
                    if back != 1 {
                        continue;
                    }
                    taken_old.insert(o);
                    taken_new.insert(c);
                    found.push(Pair {
                        old: o,
                        new: c,
                        how: "calls".into(),
                        why: format!("{what} of new {n:#x} (old {:#x})", pair.old),
                    });
                }
            }
        }
        if found.is_empty() {
            found = fill_file_gaps(old, new, &matched_old, &matched_new);
            if found.is_empty() {
                break;
            }
            progress(format!("source-file order: +{} pairs", found.len()));
        }
        progress(format!(
            "call graph: +{} pairs ({} in all)",
            found.len(),
            pairs.len() + found.len()
        ));
        frontier = found.iter().map(|p| (p.old, p.new)).collect();
        for p in found {
            matched_old.insert(p.old, p.new);
            matched_new.insert(p.new, p.old);
            pairs.insert(p.new, p);
        }
    }
    pairs.into_values().collect()
}

/// Whether two functions could be the same one in two builds: sizes within
/// a quarter of each other (or 4 instructions), and as many callees give or
/// take a quarter.
fn plausible(old: &Build, new: &Build, o: u32, n: u32) -> bool {
    if !same_file(old, new, o, n) {
        return false;
    }
    let close = |a: u32, b: u32| {
        let (lo, hi) = (a.min(b), a.max(b));
        hi - lo <= 4.max(hi / 4)
    };
    let so = old.size.get(&o).copied().unwrap_or(0);
    let sn = new.size.get(&n).copied().unwrap_or(0);
    let co = old.callees.get(&o).map_or(0, Vec::len) as u32;
    let cn = new.callees.get(&n).map_or(0, Vec::len) as u32;
    so > 0 && sn > 0 && close(so, sn) && close(co, cn)
}

/// False only when both builds name a source file for the pair and the
/// files differ.
fn same_file(old: &Build, new: &Build, o: u32, n: u32) -> bool {
    match (old.file.get(&o), new.file.get(&n)) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

/// A source path as the build machines wrote it, cut to the part under the
/// source tree: `...\src\Game\GameSim.cpp` is `game/gamesim.cpp`.
fn source_path(file: &str) -> String {
    let lower = file.to_lowercase().replace('\\', "/");
    match lower.rfind("/src/") {
        Some(i) => lower[i + 5..].to_owned(),
        None => lower,
    }
}

/// Between two matched functions of one source file, the unmatched
/// functions in the gap, in address order, pair one to one when both gaps
/// hold as many and every pair is plausible. The linker keeps a
/// translation unit's functions together and in order.
fn fill_file_gaps(
    old: &Build,
    new: &Build,
    matched_old: &HashMap<u32, u32>,
    matched_new: &HashMap<u32, u32>,
) -> Vec<Pair> {
    let group = |b: &Build| {
        let mut by_file: HashMap<String, Vec<u32>> = HashMap::new();
        for (f, file) in &b.file {
            by_file.entry(file.clone()).or_default().push(*f);
        }
        for list in by_file.values_mut() {
            list.sort_unstable();
        }
        by_file
    };
    let (by_file_old, by_file_new) = (group(old), group(new));
    let mut found = Vec::new();
    for (file, os) in &by_file_old {
        let Some(ns) = by_file_new.get(file) else {
            continue;
        };
        let pos_new: HashMap<u32, usize> = ns.iter().enumerate().map(|(i, f)| (*f, i)).collect();
        // Anchors in this file, in old address order, kept only where the
        // new side's order agrees.
        let mut anchors: Vec<(usize, usize)> = Vec::new();
        for (i, o) in os.iter().enumerate() {
            if let Some(n) = matched_old.get(o)
                && let Some(&j) = pos_new.get(n)
                && anchors.last().is_none_or(|&(_, lj)| j > lj)
            {
                anchors.push((i, j));
            }
        }
        for w in anchors.windows(2) {
            let ((i0, j0), (i1, j1)) = (w[0], w[1]);
            let gap_old: Vec<u32> = os[i0 + 1..i1]
                .iter()
                .copied()
                .filter(|f| !matched_old.contains_key(f))
                .collect();
            let gap_new: Vec<u32> = ns[j0 + 1..j1]
                .iter()
                .copied()
                .filter(|f| !matched_new.contains_key(f))
                .collect();
            if gap_old.is_empty()
                || gap_old.len() != gap_new.len()
                || !gap_old
                    .iter()
                    .zip(&gap_new)
                    .all(|(&o, &n)| plausible(old, new, o, n))
            {
                continue;
            }
            for (o, n) in gap_old.into_iter().zip(gap_new) {
                found.push(Pair {
                    old: o,
                    new: n,
                    how: "file-order".into(),
                    why: format!("in {file}, between {:#x} and {:#x}", os[i0], os[i1]),
                });
            }
        }
    }
    found
}

pub(crate) fn load(conn: &Connection) -> Result<Build> {
    let mut b = Build::default();
    let mut st = conn.prepare("SELECT rva, ninsn FROM functions")?;
    for row in st.query_map([], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?)))? {
        let (rva, n) = row?;
        b.size.insert(rva, n);
    }
    let mut st = conn.prepare(
        "SELECT caller, callee FROM calls WHERE kind IN (0, 1) AND caller != callee ORDER BY site",
    )?;
    for row in st.query_map([], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?)))? {
        let (caller, callee) = row?;
        if !b.size.contains_key(&callee) {
            continue;
        }
        let list = b.callees.entry(caller).or_default();
        if !list.contains(&callee) {
            list.push(callee);
        }
        let list = b.callers.entry(callee).or_default();
        if !list.contains(&caller) {
            list.push(caller);
        }
    }
    let mut st = conn.prepare(
        "SELECT DISTINCT s.text, x.func FROM xrefs x JOIN strings s ON s.rva = x.target \
         WHERE s.class = '' ORDER BY x.func",
    )?;
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))? {
        let (text, func) = row?;
        let list = b.strings.entry(text).or_default();
        if list.last() != Some(&func) {
            list.push(func);
        }
    }
    let mut st =
        conn.prepare("SELECT func, file FROM func_src WHERE kind IN ('direct', 'inferred')")?;
    for row in st.query_map([], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?)))? {
        let (func, file) = row?;
        b.file.insert(func, source_path(&file));
    }
    let mut st = conn.prepare(
        "SELECT v.class, v.offset, s.slot, v.nslots, s.target FROM vslots s \
         JOIN vtables v ON v.rva = s.vtable",
    )?;
    for row in st.query_map([], |r| {
        Ok((
            (
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, u32>(2)?,
                r.get::<_, u32>(3)?,
            ),
            r.get::<_, u32>(4)?,
        ))
    })? {
        b.slots.push(row?);
    }
    Ok(b)
}

/// The old build's name for a function: an exact `__FUNCSIG__` or pretty
/// name, else a unique vtable slot, else an import thunk's.
fn old_names(conn: &Connection) -> Result<HashMap<u32, String>> {
    let mut names: HashMap<u32, (u8, String)> = HashMap::new();
    let mut st = conn.prepare("SELECT addr, name, source, confidence FROM names")?;
    for row in st.query_map([], |r| {
        Ok((
            r.get::<_, u32>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })? {
        let (addr, name, source, confidence) = row?;
        let rank = match (source.as_str(), confidence.as_str()) {
            ("funcsig" | "pretty", "exact") => 3,
            ("rtti", "unique") => 2,
            ("import", _) | ("export", _) => 1,
            _ => continue,
        };
        match names.get(&addr) {
            Some((r, _)) if *r >= rank => {}
            _ => {
                names.insert(addr, (rank, name));
            }
        }
    }
    Ok(names.into_iter().map(|(a, (_, n))| (a, n)).collect())
}

pub fn run(
    old_path: &Path,
    new_path: &Path,
    dry_run: bool,
    limit: usize,
    json_out: bool,
    w: &mut dyn Write,
) -> Result<i32> {
    if old_path == new_path {
        bail!("the old and new databases are the same file");
    }
    let started = std::time::Instant::now();
    let mut progress =
        |line: String| eprintln!("[{:5.1}s] {line}", started.elapsed().as_secs_f64());
    let old_conn = db::open_ro(old_path)?;
    let new_ro = db::open_ro(new_path)?;
    progress(format!("loading {}", old_path.display()));
    let old = load(&old_conn)?;
    progress(format!("loading {}", new_path.display()));
    let new = load(&new_ro)?;
    let names = old_names(&old_conn)?;
    drop(new_ro);
    progress(format!(
        "matching {} old against {} new functions",
        old.size.len(),
        new.size.len()
    ));
    let pairs = match_builds(&old, &new, &mut progress);
    progress(format!("writing {} pairs", pairs.len()));
    let named: Vec<(&Pair, &String)> = pairs
        .iter()
        .filter_map(|p| names.get(&p.old).map(|n| (p, n)))
        .collect();
    let mut by_how: BTreeMap<String, usize> = BTreeMap::new();
    for p in &pairs {
        let key = p.how.split('(').next().unwrap_or("").to_owned();
        *by_how.entry(key).or_default() += 1;
    }
    if !dry_run {
        let mut conn = Connection::open(new_path)?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM names WHERE source = 'matched'", [])?;
        tx.execute(
            "UPDATE functions SET name = '', name_src = '' WHERE name_src = 'matched'",
            [],
        )?;
        {
            let mut ins = tx.prepare(
                "INSERT INTO names(addr, name, source, confidence, detail) VALUES (?1, ?2, 'matched', ?3, ?4)",
            )?;
            let mut best = tx.prepare(
                "UPDATE functions SET name = ?2, name_src = 'matched' WHERE rva = ?1 AND name = ''",
            )?;
            for (p, name) in &named {
                let detail = format!("old {:#x}: {}", p.old, p.why);
                ins.execute(params![p.new, name, p.how, detail])?;
                best.execute(params![p.new, format!("~{name}")])?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('matched_from', ?1)",
            [old_path.display().to_string()],
        )?;
        tx.commit()?;
    }
    if json_out {
        let items: Vec<_> = named
            .iter()
            .take(limit)
            .map(
                |(p, n)| json!({"new": p.new, "old": p.old, "name": n, "how": p.how, "why": p.why}),
            )
            .collect();
        writeln!(w, "{}", serde_json::to_string(&items)?)?;
    } else {
        writeln!(
            w,
            "matched {} functions, {} of them named in the old build ({})",
            pairs.len(),
            named.len(),
            by_how
                .iter()
                .map(|(k, v)| format!("{k} {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        )?;
        for (p, n) in named.iter().take(limit) {
            writeln!(
                w,
                "{:#x} {n} [{}] old {:#x}: {}",
                p.new, p.how, p.old, p.why
            )?;
        }
        if named.len() > limit {
            writeln!(w, "more {}", named.len() - limit)?;
        }
        if dry_run {
            writeln!(w, "dry run: nothing written")?;
        } else {
            writeln!(w, "written to {} as source 'matched'", new_path.display())?;
        }
    }
    Ok(if named.is_empty() { 1 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build of functions 1..=n with the given sizes.
    fn build(sizes: &[(u32, u32)]) -> Build {
        Build {
            size: sizes.iter().copied().collect(),
            ..Build::default()
        }
    }

    fn call(b: &mut Build, caller: u32, callee: u32) {
        b.callees.entry(caller).or_default().push(callee);
        b.callers.entry(callee).or_default().push(caller);
    }

    fn uses(b: &mut Build, f: u32, text: &str) {
        b.strings.entry(text.into()).or_default().push(f);
    }

    fn found(pairs: &[Pair]) -> Vec<(u32, u32)> {
        let mut v: Vec<_> = pairs.iter().map(|p| (p.old, p.new)).collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn a_shared_assert_string_pairs_moved_functions() {
        let mut old = build(&[(0x100, 50), (0x200, 30)]);
        let mut new = build(&[(0x900, 52), (0x700, 30)]);
        uses(&mut old, 0x100, "millis > 0 && dt > .0f");
        uses(&mut new, 0x900, "millis > 0 && dt > .0f");
        let pairs = match_builds(&old, &new, &mut |_| {});
        assert_eq!(found(&pairs), vec![(0x100, 0x900)]);
        assert_eq!(pairs[0].how, "string(1)");
    }

    #[test]
    fn a_string_many_functions_use_or_a_short_one_pairs_nothing() {
        let mut old = build(&[(1, 10), (2, 10)]);
        let mut new = build(&[(11, 10), (12, 10)]);
        for f in [1, 2] {
            uses(&mut old, f, "idx >= 0 && idx < size");
        }
        uses(&mut new, 11, "idx >= 0 && idx < size");
        uses(&mut old, 1, "true");
        uses(&mut new, 11, "true");
        assert!(match_builds(&old, &new, &mut |_| {}).is_empty());
    }

    #[test]
    fn conflicting_evidence_matches_nothing() {
        // One new function claims two old ones equally: neither is kept.
        let mut old = build(&[(1, 10), (2, 10)]);
        let mut new = build(&[(11, 10)]);
        uses(&mut old, 1, "first assert text");
        uses(&mut old, 2, "second assert text");
        uses(&mut new, 11, "first assert text");
        uses(&mut new, 11, "second assert text");
        assert!(match_builds(&old, &new, &mut |_| {}).is_empty());
    }

    #[test]
    fn rtti_slots_pair_virtual_methods_of_the_same_class() {
        let mut old = build(&[(1, 10), (2, 20)]);
        let mut new = build(&[(11, 10), (12, 20)]);
        old.slots.push((("GameState".into(), 0, 3, 8), 1));
        new.slots.push((("GameState".into(), 0, 3, 8), 11));
        // A vtable that grew is not trusted slot for slot.
        old.slots.push((("Other".into(), 0, 1, 4), 2));
        new.slots.push((("Other".into(), 0, 1, 5), 12));
        let pairs = match_builds(&old, &new, &mut |_| {});
        assert_eq!(found(&pairs), vec![(1, 11)]);
        assert_eq!(pairs[0].why, "GameState::vf3");
    }

    #[test]
    fn the_call_graph_carries_matches_to_unique_neighbours() {
        // Step (anchored) calls Tick and Sync; each has one plausible
        // partner, so both follow, and Tick's callee after it.
        let mut old = build(&[(1, 100), (2, 40), (3, 8), (4, 15)]);
        let mut new = build(&[(11, 104), (12, 41), (13, 8), (14, 16)]);
        call(&mut old, 1, 2);
        call(&mut old, 1, 3);
        call(&mut old, 2, 4);
        call(&mut new, 11, 13);
        call(&mut new, 11, 12);
        call(&mut new, 12, 14);
        uses(&mut old, 1, "m_data->totalTime >= last");
        uses(&mut new, 11, "m_data->totalTime >= last");
        let pairs = match_builds(&old, &new, &mut |_| {});
        assert_eq!(found(&pairs), vec![(1, 11), (2, 12), (3, 13), (4, 14)]);
        assert!(pairs.iter().filter(|p| p.how == "calls").count() == 3);
    }

    #[test]
    fn two_lookalike_neighbours_are_left_alone() {
        let mut old = build(&[(1, 100), (2, 20), (3, 20)]);
        let mut new = build(&[(11, 100), (12, 20), (13, 21)]);
        call(&mut old, 1, 2);
        call(&mut old, 1, 3);
        call(&mut new, 11, 12);
        call(&mut new, 11, 13);
        uses(&mut old, 1, "an assert that stays put");
        uses(&mut new, 11, "an assert that stays put");
        assert_eq!(found(&match_builds(&old, &new, &mut |_| {})), vec![(1, 11)]);
    }

    #[test]
    fn a_hubs_neighbours_are_not_compared() {
        // Both sides' hub is anchored and has more callers than the limit:
        // its callers are not matched through it.
        let n = (MAX_NEIGHBOURS + 1) as u32;
        let mut sizes: Vec<(u32, u32)> = vec![(1, 500)];
        let mut new_sizes: Vec<(u32, u32)> = vec![(10_001, 500)];
        for i in 0..n {
            sizes.push((100 + i, 10 + i * 10));
            new_sizes.push((10_100 + i, 10 + i * 10));
        }
        let (mut old, mut new) = (build(&sizes), build(&new_sizes));
        for i in 0..n {
            call(&mut old, 100 + i, 1);
            call(&mut new, 10_100 + i, 10_001);
        }
        uses(&mut old, 1, "the allocator's only assert");
        uses(&mut new, 10_001, "the allocator's only assert");
        assert_eq!(
            found(&match_builds(&old, &new, &mut |_| {})),
            vec![(1, 10_001)]
        );
    }

    #[test]
    fn functions_between_matches_in_one_source_file_pair_in_order() {
        let sizes: Vec<(u32, u32)> = (1..=5).map(|i| (i, 10 * i)).collect();
        let new_sizes: Vec<(u32, u32)> = (1..=5).map(|i| (100 + i, 10 * i)).collect();
        let (mut old, mut new) = (build(&sizes), build(&new_sizes));
        for i in 1..=5 {
            old.file.insert(i, "game/gamesim.cpp".into());
            new.file.insert(100 + i, "game/gamesim.cpp".into());
        }
        uses(&mut old, 1, "the first function's assert");
        uses(&mut new, 101, "the first function's assert");
        uses(&mut old, 5, "the last function's assert");
        uses(&mut new, 105, "the last function's assert");
        let pairs = match_builds(&old, &new, &mut |_| {});
        assert_eq!(
            found(&pairs),
            vec![(1, 101), (2, 102), (3, 103), (4, 104), (5, 105)]
        );
        assert_eq!(pairs.iter().filter(|p| p.how == "file-order").count(), 3);
    }

    #[test]
    fn a_pair_from_two_source_files_is_refused() {
        let mut old = build(&[(1, 10)]);
        let mut new = build(&[(11, 10)]);
        old.file.insert(1, "game/gamesim.cpp".into());
        new.file.insert(11, "ui/menuui.cpp".into());
        uses(&mut old, 1, "a string both happen to use");
        uses(&mut new, 11, "a string both happen to use");
        assert!(match_builds(&old, &new, &mut |_| {}).is_empty());
        assert_eq!(
            source_path(r"C:\GitLab-Runner\b\ug\train_fever\src\Game\GameSim.cpp"),
            "game/gamesim.cpp"
        );
    }
}
