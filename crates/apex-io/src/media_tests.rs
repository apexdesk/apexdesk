//! Videos and sounds of presentations through pptx and odp: the file,
//! trimming, volume, muting, looping and how they start.

use std::sync::Arc;

use apex_slides::anim::Trigger;
use apex_slides::object::{Media, Picture};
use apex_slides::{Deck, Frame, ObjKind, Object, build};

/// A 1x1 PNG.
const PNG: [u8; 70] = [
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 13, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// A video (trimmed, looped, quieter, playing by itself) and a muted
/// sound playing on a click.
fn media_deck() -> Deck {
    let mut deck = build::new_deck(0);
    let s = deck.slide_mut(0);
    let poster: Arc<[u8]> = Arc::from(&PNG[..]);
    let mut v = Media::embedded(Arc::from(&b"\x00\x00\x00\x18ftypmp42 video bytes"[..]), "video/mp4", true);
    v.trim = (1.5, 2.0);
    v.looped = true;
    v.volume = 0.5;
    v.autoplay = true;
    let a = Media { mute: true, ..Media::embedded(Arc::from(&b"ID3 sound bytes"[..]), "audio/mpeg", false) };
    for (id, m) in [(40, v), (41, a)] {
        let mut o = Object::new(id, format!("Media {id}"), ObjKind::Picture(Box::new(Picture { image: apex_doc::Image::new(poster.clone(), 100.0, 60.0), media: Some(m) })));
        o.sp.frame = Some(Frame::new(50.0, 50.0 + (id - 40) as f32 * 100.0, 160.0, 90.0));
        s.objects.push(o);
    }
    s.timing.set_media_start(40, Some(Trigger::AfterPrev), 10.0);
    deck
}

fn media_of(d: &Deck) -> Vec<(u32, Media)> {
    let mut out = Vec::new();
    for o in &d.slides[0].objects {
        if let ObjKind::Picture(p) = &o.kind
            && let Some(m) = &p.media
        {
            out.push((o.id, m.clone()));
        }
    }
    out
}

fn same(want: &[(u32, Media)], got: &[(u32, Media)], volume_tolerance: f32) {
    assert_eq!(want.len(), got.len());
    for ((_, a), (_, b)) in want.iter().zip(got) {
        assert_eq!(a.data, b.data);
        assert_eq!((a.video, &a.mime, a.trim, a.looped, a.mute, a.autoplay), (b.video, &b.mime, b.trim, b.looped, b.mute, b.autoplay));
        assert!((a.volume - b.volume).abs() < volume_tolerance, "{} {}", a.volume, b.volume);
    }
}

#[test]
fn pptx_media_round_trip() {
    let deck = media_deck();
    let want = media_of(&deck);
    assert_eq!(want.len(), 2);
    let out = crate::pptx::write(&deck);
    let back = crate::pptx::read(&out).unwrap();
    let got = media_of(&back);
    same(&want, &got, 0.01);
    assert_eq!(back.slides[0].timing.media_start(got[0].0), Some(Trigger::AfterPrev));
    assert_eq!(back.slides[0].timing.media_start(got[1].0), None);
    let z = crate::zip::ZipReader::new(&out).unwrap();
    let slide = z.read_string("ppt/slides/slide1.xml").unwrap();
    assert!(slide.contains(r#"<p14:trim st="1500" end="2000"/>"#), "{slide}");
    assert!(slide.contains(r#"cmd="playFrom(0.0)""#) && slide.contains(r#"repeatCount="indefinite""#) && slide.contains(r#"mute="1""#));
    // Unchanged, the timing goes back as read.
    same(&want, &media_of(&crate::pptx::read(&crate::pptx::write(&back)).unwrap()), 0.01);
}

#[test]
fn odp_media_round_trip() {
    let deck = media_deck();
    let want = media_of(&deck);
    let odp = crate::odp_write::write(&deck);
    let back = crate::odp::read(&odp).unwrap();
    same(&want, &media_of(&back), 0.02);
    let z = crate::zip::ZipReader::new(&odp).unwrap();
    let manifest = z.read_string("META-INF/manifest.xml").unwrap();
    assert!(manifest.contains(r#"manifest:full-path="Media/apx1.mp4" manifest:media-type="video/mp4""#), "{manifest}");
    let content = z.read_string("content.xml").unwrap();
    assert!(content.contains(r#"draw:mime-type="application/vnd.sun.star.media""#));
    // Unchanged, and then to pptx and back.
    same(&want, &media_of(&crate::odp::read(&crate::odp_write::write(&back)).unwrap()), 0.02);
    let round = crate::pptx::read(&crate::pptx::write(&back)).unwrap();
    assert_eq!(media_of(&round).len(), 2);
}

#[test]
fn linked_media_stays_linked() {
    let mut deck = media_deck();
    let s = deck.slide_mut(0);
    if let ObjKind::Picture(p) = &mut s.objects.last_mut().unwrap().kind {
        let m = p.media.as_mut().unwrap();
        m.data = None;
        m.link = Some("file:///tmp/talk.mp3".to_string());
    }
    for back in [crate::pptx::read(&crate::pptx::write(&deck)).unwrap(), crate::odp::read(&crate::odp_write::write(&deck)).unwrap()] {
        let got = media_of(&back);
        assert_eq!(got[1].1.link.as_deref(), Some("file:///tmp/talk.mp3"));
        assert!(got[1].1.data.is_none());
    }
}
