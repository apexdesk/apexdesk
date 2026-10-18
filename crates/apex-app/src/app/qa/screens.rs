//! Screenshots: every screen (`scenes`) in English at 1280×800, drawn
//! without a screen (`raster`) and compared with the pictures of the
//! last accepted run in `target/qa/baseline/` (kept out of git). A screen
//! with more than [`LIMIT`] of its pixels changed is reported, with the
//! paths of the picture before, after, and of their difference (changed
//! pixels in red). Without a baseline the run makes one and says so;
//! `scripts/qa.sh --accept-screens` makes this run's pictures the new
//! baseline.

use std::path::{Path, PathBuf};

use eframe::egui::{Color32, ColorImage};

use super::common::{self, Sink};
use super::raster;
use super::scenes::{self, WIDE};
use crate::app::audit;

/// The share of pixels a screen may change, in percent.
const LIMIT: f32 = 0.5;
/// A channel changed by less than this isn't a change (antialiasing).
const TOLERANCE: i32 = 24;
/// The screens are shared out among this many processes.
const PARTS: usize = 4;

fn baseline_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/qa/baseline"))
}

fn accepting() -> bool {
    std::env::var("APEXDESK_QA_ACCEPT_SCREENS").is_ok_and(|v| v == "1")
}

fn file_name(scene: &str) -> String {
    format!("{}.png", scene.replace(['/', ' '], "_"))
}

fn save(img: &ColorImage, p: &Path) -> Result<(), String> {
    let bytes = crate::charts::encode_png(img).ok_or("could not encode the picture")?;
    apex_fs::write(p, bytes).map_err(|e| format!("{}: {e}", p.display()))
}

fn load(p: &Path) -> Option<ColorImage> {
    let d = crate::images::decode(&apex_fs::read(p).ok()?)?;
    let px = d.rgba.chunks_exact(4).map(|p| Color32::from_rgb(p[0], p[1], p[2])).collect();
    Some(ColorImage::new([d.width as usize, d.height as usize], px))
}

/// The pixels that differ, and a picture of them: `after` dimmed, the
/// changed pixels red.
fn compare(before: &ColorImage, after: &ColorImage) -> (usize, ColorImage) {
    let mut n = 0;
    let px = before
        .pixels
        .iter()
        .zip(&after.pixels)
        .map(|(a, b)| {
            let d = [(a.r(), b.r()), (a.g(), b.g()), (a.b(), b.b())].iter().map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap_or(0);
            if d >= TOLERANCE {
                n += 1;
                Color32::from_rgb(230, 0, 0)
            } else {
                let g = ((b.r() as u32 + b.g() as u32 + b.b() as u32) / 3 / 3 + 170) as u8;
                Color32::from_rgb(g, g, g)
            }
        })
        .collect();
    (n, ColorImage::new(after.size, px))
}

fn run_part(job: &str, sink: &mut Sink) {
    audit::panic_hook();
    let Ok(part) = job.parse::<usize>() else { return };
    let base = baseline_dir();
    let shots = common::out_dir().join("screens");
    let _ = apex_fs::create_dir_all(&base);
    let _ = apex_fs::create_dir_all(&shots);
    let (mut made, mut accepted) = (Vec::new(), 0);
    for scene in scenes::all().into_iter().skip(part).step_by(PARTS) {
        audit::watch(Some(format!("screens {}", scene.name)));
        let img = match audit::quietly(|| scenes::open(&scene, WIDE).map(|rig| raster::render(&rig, WIDE))) {
            Ok(Ok(img)) => img,
            Ok(Err(why)) => {
                sink.note(format!("{}: not reached ({why})", scene.name));
                continue;
            }
            Err(e) => {
                audit::drop_context();
                crate::toolstrip::test::reset();
                sink.check();
                sink.fail(common::id(&[&scene.name, "panic"]), format!("[panic] {}: {}", scene.name, audit::panic_text(&e)));
                continue;
            }
        };
        sink.check();
        let name = file_name(&scene.name);
        let (b, after) = (base.join(&name), shots.join(&name));
        let before = if accepting() { None } else { load(&b) };
        let Some(before) = before else {
            match save(&img, &b) {
                Ok(()) if accepting() => accepted += 1,
                Ok(()) => made.push(scene.name.clone()),
                Err(e) => sink.fail(common::id(&[&scene.name, "save"]), format!("[save] {}: {e}", scene.name)),
            }
            continue;
        };
        if before.size != img.size {
            let _ = save(&img, &after);
            sink.fail(common::id(&[&scene.name, "size"]), format!("[size] {}: {:?} now, {:?} in the baseline. Before: {} After: {}", scene.name, img.size, before.size, b.display(), after.display()));
            continue;
        }
        let (n, diff) = compare(&before, &img);
        let pct = n as f32 * 100.0 / (img.size[0] * img.size[1]) as f32;
        if pct > LIMIT {
            let d = shots.join(name.replace(".png", ".diff.png"));
            let _ = save(&img, &after);
            let _ = save(&diff, &d);
            sink.fail(common::id(&[&scene.name, "changed"]), format!("[changed] {}: {pct:.2} % of the pixels changed (limit {LIMIT} %). Before: {} After: {} Diff: {}", scene.name, b.display(), after.display(), d.display()));
        }
    }
    audit::watch(None);
    if !made.is_empty() {
        sink.note(format!("no baseline for {} screens: made it now ({}), in {}", made.len(), made.join(", "), base.display()));
    }
    if accepted > 0 {
        sink.note(format!("{accepted} pictures accepted as the new baseline in {}", base.display()));
    }
}

#[test]
#[ignore]
fn qa_screens() {
    let jobs: Vec<String> = (0..PARTS).map(|i| i.to_string()).collect();
    common::run("screens", "app::qa::screens::qa_screens", &jobs, run_part);
}
