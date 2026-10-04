//! tpfre against a synthetic PE32+ built here (no game binary needed).
//!
//! The image holds hand-assembled functions chosen to hit every part of the
//! indexer and the queries, in the spirit of `tools/re/test_make_profile.py`:
//!
//! * `.pdata` functions, one of them split into two chunks by chained unwind
//!   info, and leaf functions without unwind info found as a call target, a
//!   vtable slot and a relocated data pointer;
//! * direct calls, a tail jump, an import call through the IAT, RIP-relative
//!   string and data references, a UTF-16 string, and a switch jump table in
//!   `.text`;
//! * `__FUNCSIG__` / `__FILE__` strings giving an exact name, an ambiguous
//!   one, direct and inferred source files;
//! * MSVC RTTI: a type descriptor, a complete object locator and a vtable;
//! * byte-identical twins, whose signature must be refused.

use std::path::{Path, PathBuf};

const BASE: u64 = 0x1_4000_0000;
const TEXT: u32 = 0x1000;
const RDATA: u32 = 0x2000;
const DATA: u32 = 0x3000;
const PDATA: u32 = 0x4000;
const RELOC: u32 = 0x5000;
const IMAGE_SIZE: u32 = 0x6000;
const SEC_SIZE: usize = 0x1000;
const HEADERS: usize = 0x400;

const IAT: u32 = 0x2000;
const INT: u32 = 0x2010;
const IMPDESC: u32 = 0x2020;
const DLLNAME: u32 = 0x2050;
const HINTNAME: u32 = 0x2060;
const S_SIG_ALPHA: u32 = 0x2100;
const S_FILE: u32 = 0x2140;
const S_SIG_BETA: u32 = 0x2180;
const S_T1: u32 = 0x21c0;
const S_T2: u32 = 0x2200;
const S_HELLO: u32 = 0x2240;
const U_WIDE: u32 = 0x2260;
const TD: u32 = 0x2400;
const COL: u32 = 0x2440;
const CHD: u32 = 0x2480;
const VT_META: u32 = 0x24c0;
const VT: u32 = 0x24c8;
const UNWIND: u32 = 0x2800;
const UNWIND_CHAIN: u32 = 0x2810;
const PTR_CB: u32 = 0x3000;
const PTR_STR: u32 = 0x3008;

const fn slot(i: u32) -> u32 {
    TEXT + 0x40 * i
}
const F_ALPHA: u32 = slot(0);
const F_BETA: u32 = slot(1);
const F_MIDDLE: u32 = slot(2);
const F_GAMMA: u32 = slot(3);
const F_AMB: u32 = slot(4);
const F_Y: u32 = slot(5);
const F_Z: u32 = slot(6);
const F_CTOR: u32 = slot(7);
const F_SPLIT: u32 = slot(8);
const F_SPLIT_COLD: u32 = slot(9);
const F_TWIN1: u32 = slot(10);
const F_TWIN2: u32 = slot(11);
const F_LEAF: u32 = slot(12);
const F_LEAFV: u32 = slot(13);
const F_CB: u32 = slot(14);
const F_SWITCH: u32 = slot(15);

struct Asm {
    at: u32,
    code: Vec<u8>,
}

impl Asm {
    fn new(at: u32) -> Self {
        Asm {
            at,
            code: Vec::new(),
        }
    }
    fn here(&self) -> u32 {
        self.at + self.code.len() as u32
    }
    fn raw(&mut self, hex: &str) -> &mut Self {
        let clean: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
        for i in (0..clean.len()).step_by(2) {
            self.code
                .push(u8::from_str_radix(&clean[i..i + 2], 16).expect("hex"));
        }
        self
    }
    fn rel(&mut self, opcode: &str, target: u32) -> &mut Self {
        self.raw(opcode);
        let next = self.here() + 4;
        let d = target as i64 - next as i64;
        self.code.extend_from_slice(&(d as i32).to_le_bytes());
        self
    }
}

fn functions(beta_extra_nop: bool) -> Vec<(u32, Vec<u8>, bool)> {
    // (rva, code, has .pdata entry)
    let mut out = Vec::new();
    let mut a = Asm::new(F_ALPHA);
    a.raw("4883EC28")
        .rel("488D0D", S_SIG_ALPHA)
        .rel("488D15", S_FILE)
        .rel("4C8D05", S_HELLO)
        .rel("E8", F_BETA)
        .rel("E8", F_LEAF)
        .rel("FF15", IAT)
        .raw("4883C428 C3");
    out.push((F_ALPHA, a.code, true));

    let mut a = Asm::new(F_BETA);
    a.raw("4883EC28")
        .rel("488D0D", S_SIG_BETA)
        .rel("488D15", U_WIDE);
    if beta_extra_nop {
        a.raw("90");
    }
    a.raw("4883C428").rel("E9", F_MIDDLE);
    out.push((F_BETA, a.code, true));

    let mut a = Asm::new(F_MIDDLE);
    a.raw("4883EC28 B801000000 4883C428 C3");
    out.push((F_MIDDLE, a.code, true));

    let mut a = Asm::new(F_GAMMA);
    a.raw("4883EC28").rel("488D15", S_FILE).raw("4883C428 C3");
    out.push((F_GAMMA, a.code, true));

    let mut a = Asm::new(F_AMB);
    a.raw("4883EC28")
        .rel("488D0D", S_T1)
        .rel("488D15", S_T2)
        .raw("4883C428 C3");
    out.push((F_AMB, a.code, true));

    let mut a = Asm::new(F_Y);
    a.raw("4883EC28").rel("488D0D", S_T1).raw("4883C428 C3");
    out.push((F_Y, a.code, true));

    let mut a = Asm::new(F_Z);
    a.raw("4883EC28").rel("488D0D", S_T2).raw("4883C428 C3");
    out.push((F_Z, a.code, true));

    // A constructor: stores the vtable. It is also the vtable's slot 0.
    let mut a = Asm::new(F_CTOR);
    a.rel("488D05", VT).raw("488901 C3");
    out.push((F_CTOR, a.code, true));

    // Hot part and a cold chunk (chained unwind info) that calls alpha.
    let mut a = Asm::new(F_SPLIT);
    a.raw("4883EC28 85C9 0F84")
        .code
        .extend_from_slice(&((F_SPLIT_COLD as i64 - (F_SPLIT as i64 + 12)) as i32).to_le_bytes());
    a.raw("4883C428 C3");
    out.push((F_SPLIT, a.code, true));
    let mut a = Asm::new(F_SPLIT_COLD);
    a.rel("E8", F_ALPHA).raw("4883C428 C3");
    out.push((F_SPLIT_COLD, a.code, true));

    let twin = "4883EC28 488B4110 488B4018 4883C428 C3";
    out.push((F_TWIN1, Asm::new(F_TWIN1).raw(twin).code.clone(), true));
    out.push((F_TWIN2, Asm::new(F_TWIN2).raw(twin).code.clone(), true));

    // Leaves without unwind info.
    out.push((
        F_LEAF,
        Asm::new(F_LEAF).raw("B807000000 C3").code.clone(),
        false,
    ));
    out.push((
        F_LEAFV,
        Asm::new(F_LEAFV).raw("8B4108 C3").code.clone(),
        false,
    ));
    out.push((
        F_CB,
        Asm::new(F_CB).raw("B834120000 C3").code.clone(),
        false,
    ));

    // A switch through a jump table of RVAs placed after the code.
    let mut a = Asm::new(F_SWITCH);
    a.rel("488D15", 0); // lea rdx, [__ImageBase]
    let table_at = F_SWITCH + 7 + 7 + 3 + 2 + 6 + 6 + 1; // after the code, 4-aligned
    let table_at = (table_at + 3) & !3;
    a.raw("8B8C82")
        .code
        .extend_from_slice(&table_at.to_le_bytes()); // mov ecx,[rdx+rax*4+table]
    a.raw("4803CA FFE1"); // add rcx,rdx ; jmp rcx
    let case0 = a.here();
    a.raw("B801000000 C3");
    let case1 = a.here();
    a.raw("B802000000 C3");
    while a.here() < table_at {
        a.raw("CC");
    }
    a.code.extend_from_slice(&case0.to_le_bytes());
    a.code.extend_from_slice(&case1.to_le_bytes());
    out.push((F_SWITCH, a.code, true));
    out
}

fn put(buf: &mut [u8], off: usize, bytes: &[u8]) {
    buf[off..off + bytes.len()].copy_from_slice(bytes);
}

fn build_pe(beta_extra_nop: bool) -> Vec<u8> {
    let funcs = functions(beta_extra_nop);
    let mut text = vec![0xCCu8; SEC_SIZE];
    for (rva, code, _) in &funcs {
        assert!(
            code.len() <= 0x40,
            "function at {rva:#x} overflows its slot"
        );
        put(&mut text, (*rva - TEXT) as usize, code);
    }

    let mut rdata = vec![0u8; SEC_SIZE];
    let r = |rva: u32| (rva - RDATA) as usize;
    // Imports: KERNEL32.dll!Sleep.
    put(&mut rdata, r(IAT), &(HINTNAME as u64).to_le_bytes());
    put(&mut rdata, r(INT), &(HINTNAME as u64).to_le_bytes());
    put(&mut rdata, r(IMPDESC), &INT.to_le_bytes());
    put(&mut rdata, r(IMPDESC) + 12, &DLLNAME.to_le_bytes());
    put(&mut rdata, r(IMPDESC) + 16, &IAT.to_le_bytes());
    put(&mut rdata, r(DLLNAME), b"KERNEL32.dll\0");
    put(&mut rdata, r(HINTNAME) + 2, b"Sleep\0");
    // Strings.
    put(
        &mut rdata,
        r(S_SIG_ALPHA),
        b"void __cdecl Alpha::Init(int)\0",
    );
    put(&mut rdata, r(S_FILE), b"C:\\build\\src\\game\\alpha.cpp\0");
    put(&mut rdata, r(S_SIG_BETA), b"int __cdecl Beta::Run(void)\0");
    put(&mut rdata, r(S_T1), b"void __cdecl Twin::A(void)\0");
    put(&mut rdata, r(S_T2), b"void __cdecl Twin::B(void)\0");
    put(&mut rdata, r(S_HELLO), b"Hello world\0");
    let wide: Vec<u8> = "Wide\0"
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    put(&mut rdata, r(U_WIDE), &wide);
    // RTTI: type descriptor, complete object locator, vtable.
    put(&mut rdata, r(TD), &(BASE + 0x2700).to_le_bytes());
    put(&mut rdata, r(TD) + 16, b".?AVWidget@ui@@\0");
    for (k, v) in [1u32, 0, 0, TD, CHD, COL].iter().enumerate() {
        put(&mut rdata, r(COL) + 4 * k, &v.to_le_bytes());
    }
    put(&mut rdata, r(VT_META), &(BASE + COL as u64).to_le_bytes());
    put(&mut rdata, r(VT), &(BASE + F_CTOR as u64).to_le_bytes());
    put(
        &mut rdata,
        r(VT) + 8,
        &(BASE + F_LEAFV as u64).to_le_bytes(),
    );
    // Unwind info: one plain (sub rsp,0x28), one chained back to F_SPLIT.
    put(
        &mut rdata,
        r(UNWIND),
        &[0x01, 0x04, 0x01, 0x00, 0x04, 0x42, 0x00, 0x00],
    );
    put(
        &mut rdata,
        r(UNWIND_CHAIN),
        &[0x01 | (0x4 << 3), 0x00, 0x00, 0x00],
    );
    let split_len = funcs
        .iter()
        .find(|f| f.0 == F_SPLIT)
        .map(|f| f.1.len())
        .unwrap_or(0) as u32;
    put(&mut rdata, r(UNWIND_CHAIN) + 4, &F_SPLIT.to_le_bytes());
    put(
        &mut rdata,
        r(UNWIND_CHAIN) + 8,
        &(F_SPLIT + split_len).to_le_bytes(),
    );
    put(&mut rdata, r(UNWIND_CHAIN) + 12, &UNWIND.to_le_bytes());

    let mut data = vec![0u8; SEC_SIZE];
    put(
        &mut data,
        (PTR_CB - DATA) as usize,
        &(BASE + F_CB as u64).to_le_bytes(),
    );
    put(
        &mut data,
        (PTR_STR - DATA) as usize,
        &(BASE + S_HELLO as u64).to_le_bytes(),
    );

    let mut pdata = vec![0u8; SEC_SIZE];
    let mut n = 0usize;
    for (rva, code, has) in &funcs {
        if !*has {
            continue;
        }
        let unwind = if *rva == F_SPLIT_COLD {
            UNWIND_CHAIN
        } else {
            UNWIND
        };
        put(&mut pdata, 12 * n, &rva.to_le_bytes());
        put(
            &mut pdata,
            12 * n + 4,
            &(rva + code.len() as u32).to_le_bytes(),
        );
        put(&mut pdata, 12 * n + 8, &unwind.to_le_bytes());
        n += 1;
    }

    // Base relocations for every absolute pointer.
    let mut reloc = vec![0u8; SEC_SIZE];
    let mut rl = Vec::new();
    for (page, offs) in [
        (
            RDATA,
            vec![TD - RDATA, VT_META - RDATA, VT - RDATA, VT + 8 - RDATA],
        ),
        (DATA, vec![PTR_CB - DATA, PTR_STR - DATA]),
    ] {
        let mut entries: Vec<u16> = offs.iter().map(|o| (10 << 12) | *o as u16).collect();
        if entries.len() % 2 == 1 {
            entries.push(0);
        }
        rl.extend_from_slice(&page.to_le_bytes());
        rl.extend_from_slice(&(8 + 2 * entries.len() as u32).to_le_bytes());
        for e in entries {
            rl.extend_from_slice(&e.to_le_bytes());
        }
    }
    put(&mut reloc, 0, &rl);

    // Headers.
    let mut img = vec![0u8; HEADERS];
    put(&mut img, 0, b"MZ");
    put(&mut img, 0x3c, &0x80u32.to_le_bytes());
    put(&mut img, 0x80, b"PE\0\0");
    let fh = 0x84;
    put(&mut img, fh, &0x8664u16.to_le_bytes());
    put(&mut img, fh + 2, &5u16.to_le_bytes());
    put(&mut img, fh + 4, &0x6600_0001u32.to_le_bytes());
    put(&mut img, fh + 16, &0xF0u16.to_le_bytes());
    put(&mut img, fh + 18, &0x22u16.to_le_bytes());
    let opt = fh + 20;
    put(&mut img, opt, &0x20bu16.to_le_bytes());
    put(&mut img, opt + 16, &F_ALPHA.to_le_bytes());
    put(&mut img, opt + 24, &BASE.to_le_bytes());
    put(&mut img, opt + 32, &0x1000u32.to_le_bytes());
    put(&mut img, opt + 36, &0x200u32.to_le_bytes());
    put(&mut img, opt + 56, &IMAGE_SIZE.to_le_bytes());
    put(&mut img, opt + 60, &(HEADERS as u32).to_le_bytes());
    put(&mut img, opt + 68, &3u16.to_le_bytes());
    put(&mut img, opt + 108, &16u32.to_le_bytes());
    let dir = |i: usize| opt + 112 + 8 * i;
    put(&mut img, dir(1), &IMPDESC.to_le_bytes());
    put(&mut img, dir(1) + 4, &40u32.to_le_bytes());
    put(&mut img, dir(3), &PDATA.to_le_bytes());
    put(&mut img, dir(3) + 4, &(12 * n as u32).to_le_bytes());
    put(&mut img, dir(5), &RELOC.to_le_bytes());
    put(&mut img, dir(5) + 4, &(rl.len() as u32).to_le_bytes());
    put(&mut img, dir(12), &IAT.to_le_bytes());
    put(&mut img, dir(12) + 4, &16u32.to_le_bytes());
    let secs: [(&str, u32, u32, &Vec<u8>); 5] = [
        (".text", TEXT, 0x6000_0020, &text),
        (".rdata", RDATA, 0x4000_0040, &rdata),
        (".data", DATA, 0xC000_0040, &data),
        (".pdata", PDATA, 0x4000_0040, &pdata),
        (".reloc", RELOC, 0x4200_0040, &reloc),
    ];
    let table = opt + 0xF0;
    for (i, (name, rva, ch, _)) in secs.iter().enumerate() {
        let s = table + 40 * i;
        put(&mut img, s, name.as_bytes());
        put(&mut img, s + 8, &(SEC_SIZE as u32).to_le_bytes());
        put(&mut img, s + 12, &rva.to_le_bytes());
        put(&mut img, s + 16, &(SEC_SIZE as u32).to_le_bytes());
        put(
            &mut img,
            s + 20,
            &((HEADERS + SEC_SIZE * i) as u32).to_le_bytes(),
        );
        put(&mut img, s + 36, &ch.to_le_bytes());
    }
    for (_, _, _, bytes) in secs {
        img.extend_from_slice(bytes);
    }
    img
}

struct Fixture {
    _dir: tempfile::TempDir,
    exe: PathBuf,
    db: PathBuf,
}

fn run(args: &[&str]) -> (i32, String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut argv = vec!["tpfre"];
    argv.extend_from_slice(args);
    let code = tpfre::cli::run(argv, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).expect("utf8"),
        String::from_utf8(err).expect("utf8"),
    )
}

fn fixture_with(beta_extra_nop: bool) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = dir.path().join("game.exe");
    std::fs::write(&exe, build_pe(beta_extra_nop)).expect("write fixture");
    let db = dir.path().join("game.tpfdb");
    let (code, out, err) = run(&["index", s(&exe), "-o", s(&db)]);
    assert_eq!(code, 0, "index failed: {out}{err}");
    Fixture { _dir: dir, exe, db }
}

fn fixture() -> Fixture {
    fixture_with(false)
}

fn audit_profile(exe: &Path, targets: &str) -> PathBuf {
    let dir = exe.parent().expect("parent").join("profiles");
    std::fs::create_dir_all(&dir).expect("profiles");
    let id = tpf3mp_hookcore::profile::BuildIdentity::of_file(exe).expect("identity");
    std::fs::write(
        dir.join("fixture.toml"),
        format!(
            "name = 'fixture'\nregion = '.text'\n[build]\nsha256 = '{}'\n{}",
            id.sha256, targets
        ),
    )
    .expect("profile");
    dir
}

const MIDDLE_TARGET: &str = "\n[[target]]\nname = 'middle'\nsignature = '48 83 EC 28 B8 ?? ?? ?? ?? 48 83 C4 28 C3'\nprologue = '48 83 EC 28'\n";

struct UpdateBuild {
    _root: tempfile::TempDir,
    repository: PathBuf,
    archive: PathBuf,
    bundle: PathBuf,
}

fn update_build(targets: &str) -> UpdateBuild {
    let root = tempfile::Builder::new()
        .prefix("update build with spaces ")
        .tempdir()
        .expect("tempdir");
    let game = root.path().join("game");
    let repository = root.path().join("repository");
    let bundle = repository.join("profiles/fixture-build");
    std::fs::create_dir(&game).expect("game");
    std::fs::create_dir_all(&bundle).expect("bundle");
    let bytes = build_pe(false);
    let id = tpf3mp_hookcore::profile::BuildIdentity::of_bytes(&bytes);
    std::fs::write(game.join("game.exe"), bytes).expect("game exe");
    std::fs::write(repository.join("Cargo.toml"), "[workspace]\nmembers = []\n")
        .expect("workspace");
    std::fs::write(
        repository.join("profiles/native-build.txt"),
        "fixture-build\n",
    )
    .expect("selection");
    std::fs::write(bundle.join("native.rs"), "pub const BUILD: u32 = 1;\n").expect("native data");
    std::fs::write(bundle.join("hooks.toml"), format!(
        "name = 'fixture'\nregion = '.text'\n[build]\nsha256 = '{}'\nsize = {}\npe_timestamp = {}\n{}",
        id.sha256, id.size.expect("size"), id.pe_timestamp.expect("PE timestamp"), targets,
    )).expect("profile");
    let archive = root.path().join("archive");
    tpfre::archive::create(&tpfre::archive::Options {
        game: &game,
        out: &archive,
        build: "fixture",
        executable: "game.exe",
        steam_manifest: None,
    })
    .expect("complete archive");
    UpdateBuild {
        _root: root,
        repository,
        archive,
        bundle,
    }
}

#[test]
fn build_gate_reports_the_exact_selected_profile_without_private_paths() {
    let f = update_build(MIDDLE_TARGET);
    let (code, out, err) = run(&[
        "verify-build",
        "--archive",
        s(&f.archive),
        "--repo",
        s(&f.repository),
        "--json",
    ]);
    assert_eq!(code, 0, "{out}{err}");
    let report: serde_json::Value = serde_json::from_str(&out).expect("json");
    assert_eq!(report["bundle"], "fixture-build");
    assert_eq!(report["kind"], "selected_build_verification");
    assert_eq!(report["targets"][0]["status"], "matched");
    assert_eq!(report["runtime_verified"], false);
    assert_eq!(report["review_required"], false);
    assert_eq!(report["bundle_files"].as_array().expect("files").len(), 2);
    assert!(!out.contains(s(f._root.path())));
    let selected = tpf3mp_hookcore::bundle::Bundle::selected(&f.repository.join("profiles"))
        .expect("selection");
    let module = selected.rust_module().expect("module");
    assert!(module.contains("fixture-build/hooks.toml"));
    assert!(module.contains("pub mod native;"));
}

#[test]
fn a_matching_custom_profile_cannot_bypass_the_compiled_build_gate() {
    let f = update_build(MIDDLE_TARGET);
    let hooks = std::fs::read_to_string(f.bundle.join("hooks.toml")).expect("hooks");
    let matching = tpf3mp_hookcore::profile::Profile::from_toml(&hooks).expect("profile");
    std::fs::write(f.repository.join("profiles/custom.toml"), &hooks).expect("custom profile");
    std::fs::write(
        f.bundle.join("hooks.toml"),
        hooks.replace(&matching.build.sha256, &"00".repeat(32)),
    )
    .expect("other selected build");
    assert!(
        !tpfre::audit::verify(&f.archive, &f.repository.join("profiles"))
            .expect("generic verification finds custom profile")
            .review_required
    );
    let result =
        tpfre::build_gate::build_with(&f.archive, &f.repository, None, &mut Vec::new(), |_, _| {
            panic!("must not start Cargo")
        });
    assert!(result.is_err(), "the selected native build must match");
}

#[test]
fn broken_optional_hooks_stop_the_build_even_with_an_old_success_report() {
    let targets = format!(
        "{MIDDLE_TARGET}\n[[target]]\nname = 'missing'\nrequired = false\nsignature = 'DE AD BE EF'\nprologue = 'DE'\n"
    );
    let f = update_build(&targets);
    std::fs::write(
        f.repository.join("verification.json"),
        "{\"review_required\":false}",
    )
    .expect("old report");
    let code =
        tpfre::build_gate::build_with(&f.archive, &f.repository, None, &mut Vec::new(), |_, _| {
            panic!("must not start Cargo")
        })
        .expect("target refusal report");
    assert_eq!(code, 1);
}

#[test]
fn missing_incomplete_corrupt_or_exe_only_inputs_never_start_the_build() {
    let f = update_build(MIDDLE_TARGET);
    let manifest: tpfre::archive::Manifest =
        serde_json::from_slice(&std::fs::read(f.archive.join("build.json")).expect("manifest"))
            .expect("JSON");
    for input in [
        f.archive.join("absent"),
        f.archive.join(&manifest.executable),
    ] {
        assert!(
            tpfre::build_gate::build_with(
                &input,
                &f.repository,
                None,
                &mut Vec::new(),
                |_, _| panic!("must not start Cargo")
            )
            .is_err()
        );
    }
    std::fs::write(f.archive.join(".incomplete"), "incomplete").expect("marker");
    assert!(tpfre::build_gate::verify(&f.archive, &f.repository).is_err());
    std::fs::remove_file(f.archive.join(".incomplete")).expect("remove marker");
    std::fs::write(f.archive.join(&manifest.executable), "changed").expect("corruption");
    assert!(
        tpfre::build_gate::build_with(
            &f.archive,
            &f.repository,
            None,
            &mut Vec::new(),
            |_, _| panic!("must not start Cargo")
        )
        .is_err()
    );
}

#[test]
fn build_gate_requires_a_valid_bundle_selection_and_complete_identity() {
    let f = update_build(MIDDLE_TARGET);
    let selection = f.repository.join("profiles/native-build.txt");
    for invalid in [
        "",
        "../fixture-build",
        "fixture-build/hooks.toml",
        "fixture-build\nother",
        "absent",
    ] {
        std::fs::write(&selection, invalid).expect("selection");
        assert!(
            tpfre::build_gate::verify(&f.archive, &f.repository).is_err(),
            "{invalid:?}"
        );
    }
    std::fs::write(&selection, "fixture-build").expect("restore selection");
    let hooks = std::fs::read_to_string(f.bundle.join("hooks.toml")).expect("hooks");
    let without_metadata = hooks
        .lines()
        .filter(|line| !line.starts_with("size = ") && !line.starts_with("pe_timestamp = "))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(f.bundle.join("hooks.toml"), without_metadata).expect("incomplete identity");
    assert!(tpfre::build_gate::verify(&f.archive, &f.repository).is_err());
    std::fs::write(f.bundle.join("hooks.toml"), hooks).expect("restore profile");
    std::fs::remove_file(f.bundle.join("native.rs")).expect("remove native bundle");
    assert!(tpfre::build_gate::verify(&f.archive, &f.repository).is_err());
}

#[test]
fn a_verified_build_runs_the_release_packages_and_propagates_cargo_failure() {
    let f = update_build(MIDDLE_TARGET);
    let mut called = false;
    let code = tpfre::build_gate::build_with(
        &f.archive,
        &f.repository,
        std::num::NonZeroUsize::new(2),
        &mut Vec::new(),
        |repo, args| {
            called = true;
            assert_eq!(repo, f.repository.canonicalize().expect("repository"));
            assert_eq!(
                args,
                [
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
                    "--jobs",
                    "2"
                ]
            );
            Ok(1)
        },
    )
    .expect("verified build");
    assert!(called);
    assert_eq!(code, 1, "a Cargo failure is still a failed build");
    let (code, _, _) = run(&[
        "build",
        "--archive",
        s(&f.archive),
        "--repo",
        s(&f.repository),
        "--jobs",
        "0",
    ]);
    assert_eq!(code, 2, "zero parallel jobs is a usage error");
}

#[test]
fn verification_discovers_per_build_bundles_without_parsing_their_other_metadata() {
    let f = fixture();
    let profiles = audit_profile(&f.exe, MIDDLE_TARGET);
    let bundle = profiles.join("steam-build");
    std::fs::create_dir(&bundle).expect("bundle");
    std::fs::rename(profiles.join("fixture.toml"), bundle.join("hooks.toml"))
        .expect("profile move");
    std::fs::write(bundle.join("metadata.toml"), "not a hook profile").expect("metadata");
    let (code, out, err) = run(&["verify", s(&f.exe), "--profiles", s(&profiles), "--json"]);
    assert_eq!(code, 0, "{out}{err}");
    let r: serde_json::Value = serde_json::from_str(&out).expect("json");
    assert_eq!(r["targets"].as_array().expect("targets").len(), 1);
    assert_eq!(r["targets"][0]["status"], "matched");
}

#[test]
fn undecodable_body_is_reported_unknown_without_losing_other_targets() {
    let f = fixture();
    let alpha = functions(false)
        .into_iter()
        .find(|(r, _, _)| *r == F_ALPHA)
        .expect("alpha")
        .1;
    let signature = alpha[..14]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    let targets = format!(
        "{MIDDLE_TARGET}\n[[target]]\nname = 'alpha'\nsignature = '{signature}'\nprologue = '48 83 EC 28'\n"
    );
    let profiles = audit_profile(&f.exe, &targets);
    let new = f.exe.with_file_name("invalid.exe");
    let mut bytes = build_pe(false);
    let at = HEADERS + (F_ALPHA - TEXT) as usize + 25;
    bytes[at..at + 3].copy_from_slice(&[0x60, 0x61, 0x60]); // invalid PUSHA/POPA in x64
    std::fs::write(&new, bytes).expect("new");
    let report = tpfre::audit::compare(&f.exe, &new, &profiles, &f.exe.with_file_name("cache"))
        .expect("audit still returns all targets");
    assert_eq!(report.targets.len(), 2);
    assert_eq!(report.targets[0].normalized_function_equal, Some(true));
    assert_eq!(report.targets[1].status, "matched");
    assert_eq!(report.targets[1].normalized_function_equal, None);
    assert!(
        report.targets[1]
            .comparison_error
            .as_deref()
            .is_some_and(|s| s.contains("invalid instruction"))
    );
    assert!(report.review_required);
}

#[test]
fn strict_verify_never_skips_unknown_missing_or_broken_targets() {
    let f = fixture();
    let targets = format!(
        "{MIDDLE_TARGET}\n\
        [[target]]\nname = 'optional_missing'\nrequired = false\nsignature = 'DE AD BE EF'\nprologue = 'DE'\n\
        [[target]]\nname = 'twins'\nsignature = '48 83 EC 28 48 8B 41 10 48 8B 40 18'\nprologue = '48'\n\
        [[target]]\nname = 'bad_prologue'\nsignature = '48 83 EC 28 B8 ?? ?? ?? ?? 48 83 C4 28 C3'\nprologue = '90'\n\
        [[target]]\nname = 'bad_offset'\nsignature = '48 83 EC 28 B8 ?? ?? ?? ?? 48 83 C4 28 C3'\nprologue = '48'\noffset = -100000\n"
    );
    let profiles = audit_profile(&f.exe, &targets);
    let (code, out, err) = run(&["verify", s(&f.exe), "--profiles", s(&profiles), "--json"]);
    assert_eq!(code, 1, "{out}{err}");
    let report: serde_json::Value = serde_json::from_str(&out).expect("json");
    let statuses: Vec<_> = report["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .map(|t| t["status"].as_str().expect("status"))
        .collect();
    assert_eq!(
        statuses,
        [
            "matched",
            "missing",
            "ambiguous",
            "prologue_changed",
            "out_of_bounds"
        ]
    );
    assert_eq!(report["runtime_verified"], false);

    let missing = f.exe.with_file_name("missing.exe");
    let (code, _, err) = run(&["verify", s(&missing), "--profiles", s(&profiles)]);
    assert_eq!(code, 3);
    has(&err, "missing executable");
    std::fs::write(&f.exe, build_pe(true)).expect("unknown build");
    let (code, _, err) = run(&["verify", s(&f.exe), "--profiles", s(&profiles)]);
    assert_eq!(code, 3);
    has(&err, "no profile matches");
}

#[test]
fn audit_detects_body_change_behind_a_matching_signature_and_reuses_cache() {
    let f = fixture();
    let profiles = audit_profile(&f.exe, MIDDLE_TARGET);
    let new = f.exe.with_file_name("new.exe");
    let mut bytes = build_pe(false);
    bytes[HEADERS + (F_MIDDLE - TEXT) as usize + 5] = 2;
    std::fs::write(&new, bytes).expect("new binary");
    let cache = f.exe.with_file_name("cache");
    for _ in 0..2 {
        let (code, out, err) = run(&[
            "audit",
            s(&f.exe),
            s(&new),
            "--profiles",
            s(&profiles),
            "--cache",
            s(&cache),
            "--json",
        ]);
        assert_eq!(code, 1, "{out}{err}");
        let r: serde_json::Value = serde_json::from_str(&out).expect("json");
        assert_eq!(r["targets"][0]["status"], "matched");
        assert_eq!(r["targets"][0]["normalized_function_equal"], false);
        assert_eq!(r["scripts"]["available"], false);
        assert_eq!(r["review_required"], true);
        assert_eq!(r["runtime_verified"], false);
    }
    assert_eq!(std::fs::read_dir(cache).expect("cache").count(), 2);
}

#[test]
fn complete_archives_compare_scripts_and_cannot_bypass_hash_checks() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let game = tmp.path().join("game");
    std::fs::create_dir(&game).expect("game");
    let exe = game.join("game.exe");
    std::fs::write(&exe, build_pe(false)).expect("exe");
    let profiles = audit_profile(&exe, MIDDLE_TARGET);
    std::fs::write(game.join("api.tl"), b"before").expect("script");
    let old = tmp.path().join("old");
    let new = tmp.path().join("new");
    let snapshot = |out: &Path| {
        tpfre::archive::create(&tpfre::archive::Options {
            game: &game,
            out,
            build: "test",
            executable: "game.exe",
            steam_manifest: None,
        })
        .expect("archive")
    };
    snapshot(&old);
    snapshot(&new);
    let cache = tmp.path().join("cache");
    let r = tpfre::audit::compare(&old, &new, &profiles, &cache).expect("compare");
    assert!(!r.review_required);
    assert!(r.scripts.available);
    std::fs::write(game.join("api.tl"), b"after").expect("script");
    std::fs::write(game.join("added.lua"), b"return 1").expect("new script");
    let changed = tmp.path().join("changed");
    snapshot(&changed);
    let r = tpfre::audit::compare(&old, &changed, &profiles, &cache).expect("compare");
    assert!(r.review_required);
    assert_eq!(r.scripts.changed, ["files/api.tl"]);
    assert_eq!(r.scripts.added, ["files/added.lua"]);
    std::fs::write(changed.join("files/api.tl"), b"tampered").expect("tamper");
    assert!(tpfre::audit::compare(&old, &changed, &profiles, &cache).is_err());
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf8 path")
}

fn q(f: &Fixture, args: &[&str]) -> (i32, String) {
    let mut v = vec!["q", s(&f.db)];
    v.extend_from_slice(args);
    let (code, out, err) = run(&v);
    (code, out + &err)
}

fn has(out: &str, needle: &str) {
    assert!(out.contains(needle), "expected {needle:?} in:\n{out}");
}

#[test]
fn functions_come_from_pdata_and_from_references() {
    let f = fixture();
    let (code, out) = q(&f, &["info"]);
    assert_eq!(code, 0, "{out}");
    has(&out, "count.functions_pdata 12");
    has(&out, "count.functions_discovered 3");
    has(&out, "count.jump_tables_skipped 1");
    has(&out, "count.invalid_instructions 0");
    // The leaves, each found its own way.
    has(&q(&f, &["func", "0x1300"]).1, "kind=call");
    has(&q(&f, &["func", "0x1340"]).1, "kind=vtable");
    has(&q(&f, &["func", "0x1380"]).1, "kind=pointer");
    // The chained cold chunk belongs to its primary function.
    let (_, out) = q(&f, &["func", "0x1200"]);
    has(&out, "chunk 0x1200-");
    has(&out, "chunk 0x1240-");
    has(&out, "callee 0x1000 Alpha::Init @0x1240");
}

#[test]
fn calls_tail_jumps_imports_and_callers() {
    let f = fixture();
    let (_, out) = q(&f, &["callees", "Alpha::Init"]);
    has(&out, "1 0x1000 Alpha::Init -> 0x1040 Beta::Run @0x1019");
    has(&out, "-> 0x1300 sub_1300 @0x101e");
    has(&out, "-> 0x2000 KERNEL32.dll!Sleep @0x1023 [import]");
    let (_, out) = q(&f, &["callees", "Beta::Run"]);
    has(&out, "-> 0x1080 sub_1080 @0x1056 [tail]");
    let (_, out) = q(&f, &["callers", "Alpha::Init"]);
    has(&out, "1 0x1200 sub_1200 -> 0x1000 Alpha::Init @0x1240");
    let (_, out) = q(&f, &["callers", "KERNEL32.dll!Sleep"]);
    has(&out, "0x1000 Alpha::Init -> 0x2000 KERNEL32.dll!Sleep");
    let (code, out) = q(&f, &["path", "0x1200", "0x1080"]);
    assert_eq!(code, 0, "{out}");
    has(&out, "path 3 calls");
    has(&out, "3 0x1040 Beta::Run -> 0x1080 sub_1080 @0x1056");
}

#[test]
fn strings_and_their_references() {
    let f = fixture();
    let (_, out) = q(&f, &["str", "hello"]);
    has(
        &out,
        "0x2240 \"Hello world\" <- 0x1000 Alpha::Init, data x1",
    );
    let (_, out) = q(&f, &["str", "^wide$"]);
    has(&out, "0x2260 u\"Wide\" <- 0x1040 Beta::Run");
    let (_, out) = q(&f, &["fnstr", "Alpha::Init"]);
    has(&out, "0x1004 0x2100 \"void __cdecl Alpha::Init(int)\"");
    let (_, out) = q(&f, &["xrefs", "0x2240"]);
    has(&out, "ref addr 0x1012 in 0x1000 Alpha::Init");
    has(&out, "data 0x3008");
    let (_, out) = q(&f, &["xrefs", "0x1380"]);
    has(&out, "data 0x3000");
}

#[test]
fn funcsig_and_file_naming_follow_the_python_rules() {
    let f = fixture();
    let (_, out) = q(&f, &["whois", "0x1000"]);
    has(
        &out,
        "funcsig Alpha::Init exact | void __cdecl Alpha::Init(int)",
    );
    has(&out, "file game\\alpha.cpp direct");
    // Referenced by two signatures that others also use: ambiguous.
    let (_, out) = q(&f, &["func", "0x1100"]);
    has(&out, "func 0x1100 ~Twin::A");
    has(&out, "name Twin::A funcsig ambiguous");
    has(&q(&f, &["func", "0x1140"]).1, "name Twin::A funcsig exact");
    has(&q(&f, &["func", "0x1180"]).1, "name Twin::B funcsig exact");
    // Between two functions of alpha.cpp: inferred.
    has(
        &q(&f, &["whois", "0x1080"]).1,
        "file game\\alpha.cpp inferred",
    );
    let (_, out) = q(&f, &["file", "alpha.cpp"]);
    has(&out, "file game\\alpha.cpp functions=4 direct=2");
    // A name that matches several functions is refused where one is needed.
    let (code, out) = q(&f, &["dis", "Twin::A"]);
    assert_eq!(code, 2, "{out}");
    has(&out, "ambiguous Twin::A: 2 matches");
    let (_, out) = q(&f, &["info"]);
    has(&out, "naming.functions_named_funcsig 4");
    has(&out, "naming.functions_named_ambiguous 1");
}

#[test]
fn rtti_names_classes_and_vtable_slots() {
    let f = fixture();
    let (_, out) = q(&f, &["class", "widget"]);
    has(
        &out,
        "vtable 0x24c8 ui::Widget::vftable offset=0x0 slots=2 col=0x2440",
    );
    has(&out, "slot 0 0x11c0 ui::Widget::vf0");
    has(&out, "slot 1 0x1340 ui::Widget::vf1");
    has(
        &q(&f, &["vtable", "0x24d0"]).1,
        "vtable 0x24c8 ui::Widget::vftable",
    );
    // The constructor stores the vtable.
    has(
        &q(&f, &["xrefs", "ui::Widget::vftable"]).1,
        "ref addr 0x11c0 in 0x11c0 ui::Widget::vf0",
    );
    has(
        &q(&f, &["whois", "0x2400"]).1,
        "type ui::Widget | .?AVWidget@ui@@",
    );
}

#[test]
fn signatures_are_unique_or_refused() {
    let f = fixture();
    let (code, out) = q(&f, &["sig", "Alpha::Init"]);
    assert_eq!(code, 0, "{out}");
    // Beta::Run and Twin::A open with the same 18 bytes: one more instruction.
    has(
        &out,
        "sig 0x1000 Alpha::Init section=.text sig_len=25 prologue_len=18 unique",
    );
    has(
        &out,
        "signature 48 83 EC 28 48 8D 0D ?? ?? ?? ?? 48 8D 15 ?? ?? ?? ?? 4C 8D 05 ?? ?? ?? ??",
    );
    has(
        &out,
        "prologue 48 83 EC 28 48 8D 0D F5 10 00 00 48 8D 15 2E 11 00 00",
    );
    let (_, out) = q(&f, &["sig", "Alpha::Init", "--toml"]);
    has(
        &out,
        "[[target]]\nname = \"Alpha::Init\"\nsignature = \"48 83 EC 28",
    );
    let (code, out) = q(&f, &["sig", "0x1280"]);
    assert_eq!(code, 1, "{out}");
    has(&out, "not unique (2 matches; also at RVA 0x12c0)");
    // A branch inside the bytes a far jump would steal.
    let (code, out) = q(&f, &["sig", "0x1200"]);
    assert_eq!(code, 1, "{out}");
    has(&out, "is not straight-line code");
    let (code, out) = q(&f, &["sig", "0x1200", "--steal", "5"]);
    assert_eq!(code, 0, "{out}");
    let (_, out) = q(&f, &["bytes", "48 83 EC 28 48 8D 0D ?? ?? ?? ?? 48 8D 15"]);
    has(&out, "matches 3");
    has(&out, "0x1100 in 0x1100 ~Twin::A +0x0");
}

#[test]
fn disassembly_names_targets_and_skips_jump_tables() {
    let f = fixture();
    let (_, out) = q(&f, &["dis", "Alpha::Init"]);
    has(&out, "call    Beta::Run");
    has(&out, "call    qword ptr [KERNEL32.dll!Sleep]");
    has(&out, "; \"Hello world\"");
    let (_, out) = q(&f, &["dis", "0x13c0"]);
    has(&out, "jump table 8 bytes");
    let (code, out) = q(&f, &["callees", "0x13c0"]);
    assert_eq!(code, 1, "the table must not decode as calls: {out}");
    let (_, out) = q(&f, &["dis", "0x1012", "--context", "1"]);
    has(&out, ">> 0x1012");
    assert_eq!(out.lines().count(), 4, "{out}");
}

#[test]
fn json_output_carries_the_same_facts() {
    let f = fixture();
    let (code, out) = q(&f, &["func", "Alpha::Init", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let facts = v.as_array().expect("array");
    assert_eq!(facts[0]["fact"], "func");
    assert_eq!(facts[0]["func"], "0x1000");
    assert!(
        facts
            .iter()
            .any(|x| x["fact"] == "import" && x["name"] == "KERNEL32.dll!Sleep")
    );
}

#[test]
fn a_different_binary_is_refused() {
    let f = fixture();
    let other = f.exe.with_file_name("patched.exe");
    let mut bytes = std::fs::read(&f.exe).expect("read");
    bytes[0x400 + 0x3f] ^= 0xff; // one byte of padding in .text
    std::fs::write(&other, &bytes).expect("write");
    let (code, out) = q(&f, &["--bin", s(&other), "info"]);
    assert_eq!(code, 3, "{out}");
    has(&out, "refusing");
    // The recorded binary changed on disk: byte commands refuse it too.
    std::fs::write(&f.exe, &bytes).expect("write");
    let (code, out) = q(&f, &["dis", "Alpha::Init"]);
    assert_eq!(code, 3, "{out}");
    has(&out, "refusing");
    // Commands that need only the database still answer.
    assert_eq!(q(&f, &["func", "Alpha::Init"]).0, 0);
}

#[test]
fn other_formats_are_refused_with_a_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let elf = dir.path().join("game");
    let mut bytes = b"\x7fELF\x02\x01\x01".to_vec();
    bytes.resize(256, 0);
    std::fs::write(&elf, bytes).expect("write");
    let (code, _, err) = run(&["index", s(&elf), "-o", s(&dir.path().join("x.tpfdb"))]);
    assert_eq!(code, 3);
    assert!(err.contains("ELF images are not supported yet"), "{err}");
    assert!(!dir.path().join("x.tpfdb").exists());
}

#[test]
fn truncated_images_are_refused_or_indexed_without_panicking() {
    let full = build_pe(false);
    let dir = tempfile::tempdir().expect("tempdir");
    for len in [2, 0x40, 0x90, 0x180, 0x300, 0x600, 0x1500, 0x3000] {
        let exe = dir.path().join(format!("cut{len:x}.exe"));
        std::fs::write(&exe, &full[..len.min(full.len())]).expect("write");
        let db = dir.path().join(format!("cut{len:x}.tpfdb"));
        let (code, out, err) = run(&["index", s(&exe), "-o", s(&db)]);
        assert!(code == 0 || code == 3, "{len:#x}: {code} {out}{err}");
    }
}

#[test]
fn a_memory_dump_is_read_at_its_load_address() {
    // Lay the image out as it sits in memory (sections at their RVAs) and
    // relocate its absolute pointers to another base, as the loader would.
    let file = build_pe(false);
    let load: u64 = 0x7ff6_1230_0000;
    let mut dump = vec![0u8; IMAGE_SIZE as usize];
    dump[..HEADERS].copy_from_slice(&file[..HEADERS]);
    for (i, rva) in [TEXT, RDATA, DATA, PDATA, RELOC].iter().enumerate() {
        let from = HEADERS + SEC_SIZE * i;
        dump[*rva as usize..*rva as usize + SEC_SIZE].copy_from_slice(&file[from..from + SEC_SIZE]);
    }
    for at in [TD, VT_META, VT, VT + 8, PTR_CB, PTR_STR] {
        let a = at as usize;
        let v = u64::from_le_bytes(dump[a..a + 8].try_into().expect("8 bytes"));
        dump[a..a + 8].copy_from_slice(&(v - BASE + load).to_le_bytes());
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = dir.path().join("game.dump");
    std::fs::write(&bin, &dump).expect("write");
    let db = dir.path().join("dump.tpfdb");
    let (code, out, err) = run(&[
        "index",
        s(&bin),
        "-o",
        s(&db),
        "--image-base",
        "0x7ff612300000",
    ]);
    assert_eq!(code, 0, "{out}{err}");
    let f = Fixture {
        _dir: dir,
        exe: bin,
        db,
    };
    let (_, out) = q(&f, &["info"]);
    has(&out, "dump 1");
    has(&out, "image_base 0x7ff612300000");
    has(
        &q(&f, &["class", "widget"]).1,
        "slot 1 0x1340 ui::Widget::vf1",
    );
    has(&q(&f, &["xrefs", "0x1380"]).1, "data 0x3000");
    // VAs at the load address resolve to the same RVAs.
    has(
        &q(&f, &["whois", "0x7ff612301000"]).1,
        "funcsig Alpha::Init exact",
    );
    has(&q(&f, &["dis", "Alpha::Init"]).1, "call    Beta::Run");
    let (code, out) = q(&f, &["sig", "Alpha::Init"]);
    assert_eq!(code, 0, "{out}");
    has(
        &out,
        "signature 48 83 EC 28 48 8D 0D ?? ?? ?? ?? 48 8D 15 ?? ?? ?? ?? 4C 8D 05 ?? ?? ?? ??",
    );
}

#[test]
fn diff_reports_what_changed_between_builds() {
    let a = fixture();
    let b = fixture_with(true);
    let (code, out, err) = run(&["diff", s(&a.db), s(&b.db)]);
    assert_eq!(code, 0, "{err}");
    has(
        &out,
        "summary unchanged=4 moved=0 resized=1 appeared=0 disappeared=0",
    );
    has(
        &out,
        "resized Beta::Run game\\alpha.cpp 0x1040->0x1040 size 27->28",
    );
}

fn update_options<'a>(f: &'a UpdateBuild, output: &'a Path) -> tpfre::update::Options<'a> {
    tpfre::update::Options {
        old: &f.archive,
        input: tpfre::update::Input::Archive(&f.archive),
        repository: &f.repository,
        output,
        cache: None,
        jobs: std::num::NonZeroUsize::new(2),
        check_only: false,
    }
}

fn update_summary(output: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(output.join("summary.json")).expect("summary"))
        .expect("summary JSON")
}

#[test]
fn one_update_run_snapshots_the_install_then_checks_tests_and_builds_in_order() {
    let f = update_build(MIDDLE_TARGET);
    let output = f._root.path().join("update run with spaces");
    let game = f._root.path().join("game");
    let before = std::fs::read(game.join("game.exe")).expect("original game");
    let mut options = update_options(&f, &output);
    options.input = tpfre::update::Input::Install {
        game: &game,
        build: "fixture",
        executable: "game.exe",
        steam_manifest: None,
    };
    let mut commands = Vec::new();
    let code = tpfre::update::run_with(&options, &mut Vec::new(), |repo, args, log| {
        assert_eq!(repo, f.repository.canonicalize().expect("repo"));
        assert!(log.starts_with(output.canonicalize().expect("output")));
        commands.push(args.to_vec());
        Ok(0)
    })
    .expect("pipeline");
    assert_eq!(code, 0);
    assert_eq!(
        commands
            .iter()
            .map(|args| args[0].as_str())
            .collect::<Vec<_>>(),
        ["fmt", "clippy", "test", "build"]
    );
    assert!(commands[1].ends_with(&["--".into(), "-D".into(), "warnings".into()]));
    assert!(commands[2].contains(&"--locked".into()));
    assert!(commands[2].contains(&"--no-fail-fast".into()));
    assert!(commands[3].contains(&"--release".into()));
    assert!(tpfre::archive::verify(&output.join("archive")).is_ok());
    assert_eq!(
        std::fs::read(game.join("game.exe")).expect("unchanged game"),
        before
    );
    let summary = update_summary(&output);
    assert_eq!(summary["status"], "built");
    assert_eq!(summary["runtime_verified"], false);
    assert_eq!(summary["candidate_targets"], 1);
    assert!(output.join("audit.json").is_file());
    assert!(output.join("native-bundle.json").is_file());
}

#[test]
fn a_matching_candidate_keeps_an_unreviewed_update_out_of_cargo() {
    let f = update_build(MIDDLE_TARGET);
    let game = f._root.path().join("game");
    let mut bytes = std::fs::read(game.join("game.exe")).expect("game");
    // Identity changes while .text (and its old signature) remains unchanged.
    bytes[0x88..0x8c].copy_from_slice(&12345u32.to_le_bytes());
    std::fs::write(game.join("game.exe"), &bytes).expect("updated game");
    let snapshot = f._root.path().join("updated archive");
    tpfre::archive::create(&tpfre::archive::Options {
        game: &game,
        out: &snapshot,
        build: "next",
        executable: "game.exe",
        steam_manifest: None,
    })
    .expect("new archive");
    let old_id =
        tpf3mp_hookcore::profile::BuildIdentity::of_file(&f.archive.join("files/game.exe"))
            .expect("old id");
    let new_id = tpf3mp_hookcore::profile::BuildIdentity::of_bytes(&bytes);
    let candidate = std::fs::read_to_string(f.bundle.join("hooks.toml"))
        .expect("old profile")
        .replace(&old_id.sha256, &new_id.sha256)
        .replace(
            &format!("pe_timestamp = {}", old_id.pe_timestamp.expect("timestamp")),
            &format!("pe_timestamp = {}", new_id.pe_timestamp.expect("timestamp")),
        );
    std::fs::write(f.repository.join("profiles/candidate.toml"), candidate).expect("candidate");
    let output = f._root.path().join("unreviewed run");
    let mut options = update_options(&f, &output);
    options.input = tpfre::update::Input::Archive(&snapshot);
    assert_eq!(
        tpfre::update::run_with(&options, &mut Vec::new(), |_, _, _| {
            panic!("a candidate cannot start Cargo")
        })
        .expect("diagnostic run"),
        1
    );
    let summary = update_summary(&output);
    assert_eq!(summary["status"], "blocked");
    assert_eq!(summary["stage"], "native-bundle");
    assert_eq!(summary["candidate_targets"], 1);
    assert!(output.join("audit.json").is_file());
    assert!(output.join("signatures.json").is_file());
    assert_eq!(
        std::fs::read_to_string(f.repository.join("profiles/native-build.txt")).expect("selection"),
        "fixture-build\n"
    );
}

#[test]
fn failed_tests_stop_the_update_before_a_release_build() {
    let f = update_build(MIDDLE_TARGET);
    let output = f._root.path().join("failed tests");
    let mut commands = Vec::new();
    let code = tpfre::update::run_with(
        &update_options(&f, &output),
        &mut Vec::new(),
        |_, args, _| {
            commands.push(args[0].clone());
            Ok(i32::from(args[0] == "test"))
        },
    )
    .expect("pipeline");
    assert_eq!(code, 1);
    assert_eq!(commands, ["fmt", "clippy", "test"]);
    assert_eq!(update_summary(&output)["stage"], "tests");
    assert!(
        update_summary(&output)["reason"]
            .as_str()
            .expect("reason")
            .contains("tests.log")
    );
}

#[test]
fn native_data_changed_during_tests_stops_the_update_build() {
    let f = update_build(MIDDLE_TARGET);
    let output = f._root.path().join("changing native data");
    let code = tpfre::update::run_with(
        &update_options(&f, &output),
        &mut Vec::new(),
        |_, args, _| {
            assert_ne!(args[0], "build", "changed native data must never compile");
            if args[0] == "test" {
                std::fs::write(f.bundle.join("native.rs"), "pub const BUILD: u32 = 2;\n")?;
            }
            Ok(0)
        },
    )
    .expect("pipeline");
    assert_eq!(code, 1);
    assert!(
        update_summary(&output)["reason"]
            .as_str()
            .expect("reason")
            .contains("changed during")
    );
}

#[test]
fn an_unchanged_exe_does_not_auto_approve_changed_scripts_or_libraries() {
    for (name, data) in [
        ("game.lua", "return 2"),
        ("dependency.dll", "updated library"),
    ] {
        let f = update_build(MIDDLE_TARGET);
        let output = f._root.path().join("changed payload run");
        let game = f._root.path().join("game");
        std::fs::write(game.join(name), data).expect("changed game input");
        let mut options = update_options(&f, &output);
        options.input = tpfre::update::Input::Install {
            game: &game,
            build: "next",
            executable: "game.exe",
            steam_manifest: None,
        };
        assert_eq!(
            tpfre::update::run_with(&options, &mut Vec::new(), |_, _, _| panic!(
                "changed payload requires review"
            ))
            .expect("pipeline"),
            1
        );
        let summary = update_summary(&output);
        assert!(
            summary["problems"]
                .as_array()
                .expect("problems")
                .iter()
                .any(|problem| problem["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("scripts/libraries changed")))
        );
        let files: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.join("files.json")).expect("file diff"))
                .expect("file report");
        assert!(
            files["added"]
                .as_array()
                .expect("added")
                .iter()
                .any(|path| path.as_str().is_some_and(|path| path.ends_with(name)))
        );
        assert_eq!(summary["runtime_verified"], false);
    }
}

#[test]
fn update_outputs_never_overwrite_previous_runs_or_write_in_the_game() {
    let f = update_build(MIDDLE_TARGET);
    let output = f._root.path().join("existing run");
    std::fs::create_dir(&output).expect("run");
    std::fs::write(output.join("keep"), "previous result").expect("sentinel");
    assert!(
        tpfre::update::run_with(
            &update_options(&f, &output),
            &mut Vec::new(),
            |_, _, _| panic!("invalid output")
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(output.join("keep")).expect("retained"),
        "previous result"
    );
    let inside = f.archive.join("update");
    assert!(
        tpfre::update::run_with(
            &update_options(&f, &inside),
            &mut Vec::new(),
            |_, _, _| panic!("invalid output")
        )
        .is_err()
    );
    assert!(!inside.exists());
}

#[test]
fn update_cli_has_a_static_only_mode_and_refuses_missing_snapshot_arguments() {
    let f = update_build(MIDDLE_TARGET);
    let output = f._root.path().join("static only");
    let (code, stdout, stderr) = run(&[
        "update",
        "--old",
        s(&f.archive),
        "--new",
        s(&f.archive),
        "--repo",
        s(&f.repository),
        "--out",
        s(&output),
        "--check-only",
    ]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(update_summary(&output)["status"], "static_checks_passed");
    assert!(!output.join("tests.log").exists());
    let game = f._root.path().join("game");
    let (code, _, _) = run(&[
        "update",
        "--old",
        s(&f.archive),
        "--game",
        s(&game),
        "--out",
        s(&output),
    ]);
    assert_eq!(
        code, 2,
        "an install requires its build label before snapshotting"
    );
}

#[test]
fn rejected_update_destinations_never_create_directories_in_an_input() {
    let f = update_build(MIDDLE_TARGET);
    let game = f._root.path().join("game");
    let forbidden = game.join("do-not-create");
    let output = forbidden.join("run");
    let mut options = update_options(&f, &output);
    options.input = tpfre::update::Input::Install {
        game: &game,
        build: "fixture",
        executable: "game.exe",
        steam_manifest: None,
    };
    assert!(
        tpfre::update::run_with(&options, &mut Vec::new(), |_, _, _| panic!(
            "invalid output"
        ))
        .is_err()
    );
    assert!(!forbidden.exists());
    let output = f._root.path().join("safe run");
    let mut options = update_options(&f, &output);
    options.input = tpfre::update::Input::Install {
        game: &game,
        build: "fixture",
        executable: "game.exe",
        steam_manifest: None,
    };
    options.cache = Some(&forbidden);
    assert!(
        tpfre::update::run_with(&options, &mut Vec::new(), |_, _, _| panic!("invalid cache"))
            .is_err()
    );
    assert!(!forbidden.exists());
    assert!(!output.exists());
}

#[test]
fn one_analysis_collects_both_missing_profile_and_native_bundle_problems() {
    let f = update_build(MIDDLE_TARGET);
    let game = f._root.path().join("game");
    let mut bytes = std::fs::read(game.join("game.exe")).expect("game");
    bytes[0x88..0x8c].copy_from_slice(&999u32.to_le_bytes());
    std::fs::write(game.join("game.exe"), bytes).expect("updated game");
    let output = f._root.path().join("all problems run");
    let mut options = update_options(&f, &output);
    options.input = tpfre::update::Input::Install {
        game: &game,
        build: "next",
        executable: "game.exe",
        steam_manifest: None,
    };
    assert_eq!(
        tpfre::update::run_with(&options, &mut Vec::new(), |_, _, _| panic!("unknown build"))
            .expect("pipeline"),
        1
    );
    let summary = update_summary(&output);
    let problems = summary["problems"].as_array().expect("all problems");
    assert_eq!(problems.len(), 2);
    assert_eq!(problems[0]["stage"], "signatures");
    assert_eq!(problems[1]["stage"], "native-bundle");
    assert!(output.join("files.json").is_file());
    assert!(output.join("audit.json").is_file());
}
