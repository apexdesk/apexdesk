//! Videos and sounds: decoding with the system's media framework
//! ([`decode`]), playing ([`player`]) and the system's sound output
//! ([`audio_out`]). Without one a video shows its poster picture.

mod audio_out;
pub mod decode;
#[cfg(feature = "ffmpeg")]
mod ffmpeg;
#[cfg(all(unix, not(target_os = "macos")))]
mod gst;
#[cfg(windows)]
mod mf;
#[cfg(target_os = "macos")]
mod avf;
pub mod player;

pub use player::Player;

/// Media type of a video or sound file by its name, and whether it's a video.
pub fn mime_of(name: &str) -> Option<(&'static str, bool)> {
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "mp4" | "m4v" => ("video/mp4", true),
        "mov" => ("video/quicktime", true),
        "mkv" => ("video/x-matroska", true),
        "webm" => ("video/webm", true),
        "avi" => ("video/x-msvideo", true),
        "wmv" => ("video/x-ms-wmv", true),
        "ogv" => ("video/ogg", true),
        "mp3" => ("audio/mpeg", false),
        "m4a" | "aac" => ("audio/mp4", false),
        "wav" => ("audio/wav", false),
        "ogg" | "oga" | "opus" => ("audio/ogg", false),
        "flac" => ("audio/flac", false),
        "wma" => ("audio/x-ms-wma", false),
        _ => return None,
    })
}

/// File name extensions offered when inserting.
pub const VIDEO_EXTS: [&str; 7] = ["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv"];
pub const AUDIO_EXTS: [&str; 7] = ["mp3", "m4a", "wav", "ogg", "opus", "flac", "wma"];

/// "m:ss" of seconds.
pub fn clock_text(s: f64) -> String {
    let s = s.max(0.0);
    let m = (s / 60.0).floor();
    format!("{}:{:04.1}", m as u64, s - m * 60.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const CLIP: &[u8] = include_bytes!("../tests/data/clip.mp4");

    #[test]
    fn media_types() {
        assert_eq!(mime_of("a/Talk.MP4"), Some(("video/mp4", true)));
        assert_eq!(mime_of("x.flac"), Some(("audio/flac", false)));
        assert_eq!(mime_of("x.txt"), None);
        assert_eq!(clock_text(75.25), "1:15.2");
    }

    /// With the system's decoders (GStreamer with its good plugins and an
    /// H.264 and AAC decoder, on Linux), or FFmpeg in a build with it;
    /// else there's nothing to check.
    #[test]
    fn decodes_a_clip() {
        if !decode::available() {
            eprintln!("no media support: skipped");
            return;
        }
        let src = decode::Source::Bytes(Arc::from(CLIP));
        let Ok(info) = decode::probe(&src) else {
            eprintln!("{}: can't decode H.264 and AAC here: skipped", decode::SYSTEM);
            return;
        };
        assert_eq!(info.size, Some((64, 48)));
        assert!(info.audio);
        assert!((info.duration - 2.0).abs() < 0.2, "{}", info.duration);
        let (w, h, rgba) = decode::picture_at(&src, 1.0, 32).unwrap();
        assert_eq!((w, h, rgba.len()), (32, 24, 32 * 24 * 4));
        let (png, ..) = player::poster_png(&src, 0.0).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        // All of it: 20 pictures and 2 seconds of sound.
        let mut d = decode::Decoder::open(&src, true).unwrap();
        d.out_rate = 8000;
        let (mut pictures, mut samples, mut last) = (0, 0, 0.0);
        while let Some(o) = d.next() {
            match o {
                decode::Out::Video { t, .. } => {
                    pictures += 1;
                    last = t;
                }
                decode::Out::Audio { samples: s, .. } => samples += s.len() / 2,
            }
        }
        assert_eq!(pictures, 20);
        assert!((last - 1.9).abs() < 0.01, "{last}");
        assert!((15_000..17_500).contains(&samples), "{samples}");
        // Back to a time after the end (looping, going back): pictures from there.
        d.seek(1.0);
        let t = std::iter::from_fn(|| d.next()).find_map(|o| match o {
            decode::Out::Video { t, .. } => Some(t),
            _ => None,
        });
        assert!(t.is_some_and(|t| (0.0..=1.05).contains(&t)), "{t:?}");
        // Only the sound, from a file on disk.
        let dir = std::env::temp_dir().join(format!("apexdesk-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clip.mp4");
        std::fs::write(&path, CLIP).unwrap();
        let mut d = decode::Decoder::open(&decode::Source::Path(path), false).unwrap();
        assert_eq!(d.info().size, None);
        d.out_rate = 8000;
        assert!(matches!(d.next(), Some(decode::Out::Audio { .. })));
        drop(d);
        let _ = std::fs::remove_dir_all(&dir);
        // Damaged files are refused, not crashed on.
        assert!(decode::Decoder::open(&decode::Source::Bytes(Arc::from(&CLIP[..400])), true).is_err() || decode::probe(&decode::Source::Bytes(Arc::from(&CLIP[..400]))).is_ok());
    }
}
