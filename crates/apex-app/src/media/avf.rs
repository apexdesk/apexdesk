//! AVFoundation, macOS's media framework: not wired up yet, so on macOS
//! videos show their poster picture (as without any media support). The
//! plan: an AVAssetReader with two track outputs, 32BGRA pictures and
//! interleaved float sound, converted as Media Foundation's are.

use super::decode::{Backend, Source};

pub fn load() -> Result<(), String> {
    Err("not available in this build".into())
}

pub fn open(_src: &Source, _want_video: bool) -> Result<Box<dyn Backend>, String> {
    Err("not available in this build".into())
}
