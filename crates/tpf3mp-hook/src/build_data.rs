//! The compiled native ABI and its exact executable identity.
//!
//! Changing a TOML profile cannot opt a new executable into old native layouts.
//! Each supported native build is reviewed as a directory under `profiles/`.

use tpf3mp_hookcore::profile::{BuildIdentity, Profile};

// Generated from profiles/native-build.txt, the same selection checked before
// packaging. A new build explicitly selects its reviewed native bundle there.
include!(concat!(env!("OUT_DIR"), "/native_bundle.rs"));

pub const BUILT_IN_PROFILES: &[(&str, &str)] = &[(native::PROFILE_NAME, native::PROFILE_TOML)];

/// Required before any menu, step gate or optional native fix is installed.
pub fn verify_identity(actual: &BuildIdentity) -> Result<(), String> {
    let profile = Profile::from_toml(native::PROFILE_TOML)
        .map_err(|e| format!("invalid compiled native bundle: {e}"))?;
    // Require the metadata recorded by this bundle, not merely the optional
    // comparisons made when a generic profile is used for analysis.
    if actual.sha256 != profile.build.sha256
        || actual.size != profile.build.size
        || actual.pe_timestamp != profile.build.pe_timestamp
    {
        return Err(format!(
            "no compiled native bundle matches build {}; profiles alone cannot supply native layouts (compiled: {})",
            actual.sha256,
            native::PROFILE_NAME
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_profile_cannot_enable_a_new_build_with_old_native_layouts() {
        let profile = Profile::from_toml(native::PROFILE_TOML).expect("compiled profile");
        assert!(verify_identity(&profile.build).is_ok());
        let mut unknown = profile.build.clone();
        unknown.sha256 = "01".repeat(32);
        // An externally supplied profile can declare this new identity.
        let replacement = native::PROFILE_TOML.replace(&profile.build.sha256, &unknown.sha256);
        let external = Profile::from_toml(&replacement).expect("external profile");
        assert!(external.verify_identity(&unknown).is_ok());
        assert!(verify_identity(&unknown).is_err());
        unknown = profile.build.clone();
        unknown.size = unknown.size.map(|v| v + 1);
        assert!(verify_identity(&unknown).is_err());
        unknown = profile.build.clone();
        unknown.pe_timestamp = None;
        assert!(verify_identity(&unknown).is_err());
    }
}
