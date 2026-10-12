//! Business licenses are part of ApexDesk Empresa; the free build has
//! none, so nothing they unlock is there.

/// Whether a license in force unlocks `feature`: never in the free build.
pub fn has(_feature: &str) -> bool {
    false
}
