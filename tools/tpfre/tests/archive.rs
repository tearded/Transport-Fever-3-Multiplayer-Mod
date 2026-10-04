//! Snapshot safety and actual ZIP/UG decompression, without proprietary inputs.
use std::{fs, io::Write, path::Path};
use tpfre::archive::{self, Options};

fn zip(name: &str, data: &[u8], ug: bool, deflate: bool, bad_crc: bool) -> Vec<u8> {
    let compressed = if deflate {
        let mut d = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        d.write_all(data).expect("compress");
        d.finish().expect("finish")
    } else {
        data.to_vec()
    };
    let method = if deflate { 8u16 } else { 0 };
    let crc = crc32fast::hash(data) ^ u32::from(bad_crc);
    let mut b = Vec::new();
    b.extend_from_slice(if ug { b"UG\x03\x04" } else { b"PK\x03\x04" });
    b.extend_from_slice(&20u16.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&method.to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&crc.to_le_bytes());
    b.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&(name.len() as u16).to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(name.as_bytes());
    b.extend_from_slice(&compressed);
    let start = b.len() as u32;
    b.extend_from_slice(b"PK\x01\x02");
    b.extend_from_slice(&20u16.to_le_bytes());
    b.extend_from_slice(&20u16.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&method.to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&crc.to_le_bytes());
    b.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&(name.len() as u16).to_le_bytes());
    b.extend_from_slice(&[0; 16]);
    b.extend_from_slice(name.as_bytes());
    let size = b.len() as u32 - start;
    b.extend_from_slice(b"PK\x05\x06");
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&size.to_le_bytes());
    b.extend_from_slice(&start.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b
}

fn create(game: &Path, out: &Path) -> anyhow::Result<archive::Manifest> {
    archive::create(&Options {
        game,
        out,
        build: "42",
        executable: "game.exe",
        steam_manifest: None,
    })
}

#[test]
fn archive_copies_libraries_sources_and_both_zip_formats_without_overwriting() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let game = tmp.path().join("game");
    fs::create_dir(&game).expect("game");
    fs::write(game.join("game.exe"), b"exe").expect("exe");
    fs::write(game.join("api.dll"), b"library").expect("dll");
    fs::write(game.join("api.tl"), b"loose").expect("script");
    fs::write(game.join("assets.bin"), b"omit").expect("asset");
    fs::write(
        game.join("normal.zip"),
        zip("api/mod.lua", b"return 1", false, false, false),
    )
    .expect("zip");
    fs::write(
        game.join("game.zip"),
        zip("api/game.tl", b"return 2", true, true, false),
    )
    .expect("zip");
    let out = tmp.path().join("nested/private/42");
    let m = create(&game, &out).expect("archive");
    assert_eq!(m.files.len(), 5);
    assert_eq!(
        m.sources
            .iter()
            .find(|s| s.path == "game.zip")
            .expect("ZIP source")
            .hash_scope,
        "script_entries"
    );
    assert_eq!(
        fs::read(out.join("scripts/game.zip/api/game.tl")).expect("script"),
        b"return 2"
    );
    assert_eq!(
        fs::read(out.join("scripts/normal.zip/api/mod.lua")).expect("script"),
        b"return 1"
    );
    assert!(archive::verify(&out).is_ok());
    assert!(create(&game, &out).is_err());
    assert_eq!(fs::read(game.join("game.exe")).expect("exe"), b"exe");
    assert!(!out.join("files/assets.bin").exists());
    let digest = &m
        .sources
        .iter()
        .find(|s| s.path == "game.zip")
        .expect("ZIP source")
        .sha256;
    fs::write(
        game.join("game.zip"),
        zip("api/game.tl", b"return 2", false, false, false),
    )
    .expect("recompressed zip");
    let recompressed = create(&game, &tmp.path().join("recompressed")).expect("archive");
    assert_eq!(
        &recompressed
            .sources
            .iter()
            .find(|s| s.path == "game.zip")
            .expect("ZIP source")
            .sha256,
        digest
    );
    let mut broken: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("build.json")).expect("manifest")).expect("json");
    broken["files"]
        .as_array_mut()
        .expect("files")
        .retain(|f| f["source"] != "normal.zip");
    fs::write(
        out.join("build.json"),
        serde_json::to_vec(&broken).expect("json"),
    )
    .expect("manifest");
    assert!(archive::verify(&out).is_err());
    fs::write(
        out.join("build.json"),
        serde_json::to_vec(&m).expect("json"),
    )
    .expect("restore");
    fs::write(out.join("files/api.dll"), b"damaged").expect("tamper");
    assert!(archive::verify(&out).is_err());
}

#[test]
fn unsafe_paths_and_corrupt_scripts_leave_an_unusable_archive() {
    for (name, bad_crc) in [
        ("../escape.lua", false),
        ("C:/escape.lua", false),
        ("api.lua", true),
    ] {
        let tmp = tempfile::tempdir().expect("tempdir");
        let game = tmp.path().join("game");
        fs::create_dir(&game).expect("game");
        fs::write(game.join("game.exe"), b"exe").expect("exe");
        fs::write(
            game.join("bad.zip"),
            zip(name, b"script", true, true, bad_crc),
        )
        .expect("zip");
        let out = tmp.path().join("archive");
        assert!(create(&game, &out).is_err());
        assert!(out.join(".incomplete").exists());
        assert!(!out.join("build.json").exists());
        assert!(archive::verify(&out).is_err());
        assert!(!tmp.path().join("escape.lua").exists());
    }
}

#[test]
fn refuses_install_git_and_mismatched_steam_build_before_copying() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let game = tmp.path().join("game");
    fs::create_dir(&game).expect("game");
    fs::write(game.join("game.exe"), b"exe").expect("exe");
    assert!(create(&game, &game.join("archive")).is_err());
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).expect("repo");
    fs::write(repo.join(".git"), b"gitdir: elsewhere").expect("git");
    assert!(create(&game, &repo.join("archive")).is_err());
    let steam = tmp.path().join("appmanifest.acf");
    fs::write(&steam, b"\"buildid\" \"99\"").expect("manifest");
    let out = tmp.path().join("archive");
    assert!(
        archive::create(&Options {
            game: &game,
            out: &out,
            build: "42",
            executable: "game.exe",
            steam_manifest: Some(&steam)
        })
        .is_err()
    );
    assert!(!out.exists());
    fs::write(&steam, b"\"buildid\" \"42\"\n\"BetaKey\" \"preview\"").expect("manifest");
    archive::create(&Options {
        game: &game,
        out: &out,
        build: "42",
        executable: "game.exe",
        steam_manifest: Some(&steam),
    })
    .expect("archive");
    assert_eq!(
        fs::read(out.join("steam/appmanifest.acf")).expect("saved manifest"),
        fs::read(steam).expect("manifest")
    );
}
