//! Downloads checked against a signed size and SHA-256, shared with the
//! launcher's updater (D7): the same HTTPS client (`ureq` with rustls and
//! ring, with the `install` feature), the same limits, the same check on
//! the way in.

#[cfg(feature = "install")]
use std::time::Duration;
use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
};

use ring::digest::{Context, SHA256};
use thiserror::Error;

use crate::signed::hex;

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("cannot download: {0}")]
    Http(String),
    #[error("the download does not match its signed size and SHA-256")]
    Mismatch,
}

#[cfg(feature = "install")]
impl From<ureq::Error> for FetchError {
    fn from(error: ureq::Error) -> Self {
        Self::Http(error.to_string())
    }
}

/// An HTTP client: HTTPS only unless `https_only` is false (tests serve
/// plain HTTP on loopback).
#[cfg(feature = "install")]
pub fn agent(https_only: bool, timeout: Duration) -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .https_only(https_only)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_global(Some(timeout))
            .build(),
    )
}

/// The body at `url`, at most `limit` bytes.
#[cfg(feature = "install")]
pub fn get(
    agent: &ureq::Agent,
    url: &str,
    limit: u64,
    user_agent: &str,
) -> Result<Vec<u8>, FetchError> {
    let mut response = agent.get(url).header("User-Agent", user_agent).call()?;
    Ok(response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()?)
}

/// Downloads `url` into `path`, checking its size and hash on the way:
/// more than `size` bytes stops the download, and a file of another size
/// or hash is an error. The caller deletes `path` on an error.
#[cfg(feature = "install")]
pub fn download(
    agent: &ureq::Agent,
    url: &str,
    path: &Path,
    size: u64,
    sha256: &str,
    user_agent: &str,
    progress: impl FnMut(u64),
) -> Result<(), FetchError> {
    let mut response = agent.get(url).header("User-Agent", user_agent).call()?;
    let reader = response.body_mut().with_config().limit(size + 1).reader();
    copy_verified(reader, path, size, sha256, progress)
}

/// Copies `reader` into a new file at `path`, checking it is `size` bytes
/// hashing to `sha256`.
pub fn copy_verified(
    mut reader: impl Read,
    path: &Path,
    size: u64,
    sha256: &str,
    mut progress: impl FnMut(u64),
) -> Result<(), FetchError> {
    let mut file = File::create(path)?;
    let mut digest = Context::new(&SHA256);
    let mut buffer = vec![0; 256 * 1024];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > size {
            return Err(FetchError::Mismatch);
        }
        digest.update(&buffer[..read]);
        file.write_all(&buffer[..read])?;
        progress(total);
    }
    file.sync_all()?;
    if total != size || hex(digest.finish().as_ref()) != sha256 {
        return Err(FetchError::Mismatch);
    }
    Ok(())
}

/// Whether the file at `path` is `size` bytes hashing to `sha256`. A file
/// that cannot be opened is not.
pub fn file_matches(path: &Path, size: u64, sha256: &str) -> io::Result<bool> {
    let Ok(mut file) = File::open(path) else {
        return Ok(false);
    };
    if file.metadata()?.len() != size {
        return Ok(false);
    }
    let mut digest = Context::new(&SHA256);
    let mut buffer = vec![0; 256 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(digest.finish().as_ref()) == sha256)
}

/// The SHA-256 of the file at `path`, lowercase hex: the game build a
/// native package is pinned to, from the game's executable.
pub fn sha256_of_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Context::new(&SHA256);
    let mut buffer = vec![0; 256 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(digest.finish().as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signed::sha256_hex;

    #[test]
    fn only_the_signed_bytes_are_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        let bytes = b"native data";
        let hash = sha256_hex(bytes);
        copy_verified(&bytes[..], &path, bytes.len() as u64, &hash, |_| {}).unwrap();
        assert!(file_matches(&path, bytes.len() as u64, &hash).unwrap());
        assert_eq!(sha256_of_file(&path).unwrap(), hash);

        // Cut short, too long, or other bytes of the same size.
        for (body, size) in [
            (&bytes[..4], bytes.len()),
            (&b"native data and more"[..], bytes.len()),
            (&b"native DATA"[..], bytes.len()),
        ] {
            assert!(matches!(
                copy_verified(body, &path, size as u64, &hash, |_| {}),
                Err(FetchError::Mismatch)
            ));
        }
        assert!(!file_matches(&dir.path().join("none"), 1, &hash).unwrap());
    }
}
