//! The native bundle selected by a checkout, shared with the hook build script.
//! Keep this module std-only: the build script includes it directly.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub struct Bundle {
    pub name: String,
    pub directory: PathBuf,
}

impl Bundle {
    /// One directory name, never an arbitrary path or a fallback profile.
    pub fn selected(profiles: &Path) -> io::Result<Self> {
        let text = fs::read_to_string(profiles.join("native-build.txt"))?;
        let name = text.trim();
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native-build.txt must name one build directory",
            ));
        }
        let profiles = profiles.canonicalize()?;
        let directory = profiles.join(name).canonicalize()?;
        if !directory.starts_with(&profiles) || directory == profiles {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native bundle escapes profiles directory",
            ));
        }
        for file in ["hooks.toml", "native.rs"] {
            let path = directory.join(file).canonicalize()?;
            if !path.starts_with(&directory) || !path.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "native bundle needs local hooks.toml and native.rs files",
                ));
            }
        }
        Ok(Self {
            name: name.into(),
            directory,
        })
    }

    /// Bind both the Rust module and built-in profile to this same selection.
    pub fn rust_module(&self) -> io::Result<String> {
        let profile = self.directory.join("hooks.toml");
        let native = self.directory.join("native.rs");
        let profile = profile
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 bundle path"))?;
        let native = native
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 bundle path"))?;
        Ok(format!(
            "pub const COMPILED_PROFILE_NAME: &str = {:?};\npub const COMPILED_PROFILE_TOML: &str = include_str!({profile:?});\n#[path = {native:?}]\npub mod native;\n",
            format!("{}/hooks.toml", self.name)
        ))
    }
}
