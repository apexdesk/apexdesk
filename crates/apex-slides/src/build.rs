//! A new presentation: a 16:9 deck with one master in a built-in theme,
//! Office's usual layouts and a title slide.

use std::sync::Arc;

use apex_doc::{Align, Theme, VAnchor};

use crate::color::{ClrMap, SColor, Scheme};
use crate::fill::{Background, FormatScheme};
use crate::object::{Frame, ObjKind, Object, PhType, Placeholder};
use crate::text::{Autofit, BodyProps, Bullet, ListStyle, ParaProps, RunProps, Spacing, TextBody};
use crate::{Deck, HeaderFooter, Layout, Master, Package, ShowSettings, SlideTheme};

/// 16:9 slides (13.333 x 7.5 in), in points.
pub const WIDE: (f32, f32) = (960.0, 540.0);
/// 4:3 slides (10 x 7.5 in).
pub const STANDARD: (f32, f32) = (720.0, 540.0);

/// A placeholder object with empty text.
pub fn placeholder(id: u32, name: &str, kind: PhType, idx: u32, frame: Option<Frame>) -> Object {
    let mut o = Object::new(id, name, ObjKind::Shape { text: Some(TextBody::plain("")), text_box: false, connector: false });
    o.ph = Some(Placeholder::new(kind, idx));
    o.sp.frame = frame;
    o
}

fn run(size: f32, font: &str) -> RunProps {
    RunProps { size: Some(size), font: Some(Arc::from(font)), color: Some(SColor::scheme(Scheme::Tx1)), ..Default::default() }
}

/// Office's master text styles: titles, bodies (bulleted, five sizes)
/// and other text.
pub fn office_styles() -> (ListStyle, ListStyle, ListStyle) {
    let mut title = ListStyle::default();
    *title.level_mut(0) = ParaProps { align: Some(Align::Left), line: Some(Spacing::Pct(0.9)), before: Some(Spacing::Pct(0.0)), bullet: Some(Bullet::None), run: run(44.0, apex_doc::theme::MAJOR_TOKEN), ..Default::default() };
    let mut body = ListStyle::default();
    let sizes = [28.0, 24.0, 20.0, 18.0, 18.0, 18.0, 18.0, 18.0, 18.0];
    for (l, s) in sizes.into_iter().enumerate() {
        *body.level_mut(l as u8) = ParaProps {
            align: Some(Align::Left),
            mar_l: Some(18.0 + 36.0 * l as f32),
            indent: Some(-18.0),
            line: Some(Spacing::Pct(0.9)),
            before: Some(Spacing::Pts(if l == 0 { 10.0 } else { 5.0 })),
            bullet: Some(Bullet::Char('•')),
            bullet_font: Some(Some(Arc::from("Arial"))),
            run: run(s, apex_doc::theme::MINOR_TOKEN),
            ..Default::default()
        };
    }
    let mut other = ListStyle::default();
    other.def = Some(ParaProps::default());
    for l in 0..9u8 {
        *other.level_mut(l) = ParaProps { mar_l: Some(36.0 * l as f32), align: Some(Align::Left), run: run(18.0, apex_doc::theme::MINOR_TOKEN), ..Default::default() };
    }
    (title, body, other)
}

/// Text settings of a placeholder: its size, alignment, anchoring and bullets.
fn tune(o: &mut Object, size: Option<f32>, align: Option<Align>, anchor: Option<VAnchor>, bullets: bool) {
    let Some(t) = o.text_mut() else { return };
    let mut p = ParaProps { align, ..Default::default() };
    if !bullets {
        p.bullet = Some(Bullet::None);
        p.mar_l = Some(0.0);
        p.indent = Some(0.0);
    }
    p.run.size = size;
    *t.list.level_mut(0) = p;
    t.body = BodyProps { anchor, ..Default::default() };
}

fn master_objects(w: f32, h: f32) -> Vec<Object> {
    let (m, fh) = (w * 0.06875, h * 0.0532);
    let fy = h - fh - h * 0.0208;
    let mut title = placeholder(2, "Title Placeholder 1", PhType::Title, 0, Some(Frame::new(m, h * 0.0532, w - 2.0 * m, h * 0.1933)));
    tune(&mut title, None, None, Some(VAnchor::Middle), false);
    if let Some(t) = title.text_mut() {
        t.body.autofit = Some(Autofit::Shrink { scale: 1.0, reduce: 0.0 });
    }
    let mut body = placeholder(3, "Text Placeholder 2", PhType::Body, 1, Some(Frame::new(m, h * 0.2662, w - 2.0 * m, h * 0.6345)));
    if let Some(t) = body.text_mut() {
        t.body = BodyProps { anchor: Some(VAnchor::Top), autofit: Some(Autofit::Shrink { scale: 1.0, reduce: 0.0 }), ..Default::default() };
        t.set_plain("Click to edit Master text styles\nSecond level\nThird level\nFourth level\nFifth level");
        for (i, p) in t.paras.iter_mut().enumerate() {
            p.style.level = i as u8;
        }
    }
    let third = (w - 2.0 * m) / 3.83;
    let mut dt = placeholder(4, "Date Placeholder 3", PhType::Date, 2, Some(Frame::new(m, fy, third * 0.866, fh)));
    tune(&mut dt, Some(12.0), Some(Align::Left), Some(VAnchor::Middle), false);
    let mut ftr = placeholder(5, "Footer Placeholder 4", PhType::Footer, 3, Some(Frame::new(w * 0.33125, fy, w * 0.3375, fh)));
    tune(&mut ftr, Some(12.0), Some(Align::Center), Some(VAnchor::Middle), false);
    let mut num = placeholder(6, "Slide Number Placeholder 5", PhType::SlideNum, 4, Some(Frame::new(w - m - third * 0.866, fy, third * 0.866, fh)));
    tune(&mut num, Some(12.0), Some(Align::Right), Some(VAnchor::Middle), false);
    for o in [&mut dt, &mut ftr, &mut num] {
        let c = SColor::scheme(Scheme::Tx1).with(crate::color::Mod::Tint(75000));
        if let Some(t) = o.text_mut() {
            t.list.level_mut(0).run.color = Some(c);
        }
    }
    vec![title, body, dt, ftr, num]
}

/// Footer placeholders of a layout (they follow the master's).
fn hf(next: &mut u32) -> Vec<Object> {
    let mut v = Vec::new();
    for (kind, idx, name) in [(PhType::Date, 10, "Date Placeholder"), (PhType::Footer, 11, "Footer Placeholder"), (PhType::SlideNum, 12, "Slide Number Placeholder")] {
        *next += 1;
        v.push(placeholder(*next, &format!("{name} {}", *next - 1), kind, idx, None));
    }
    v
}

fn layout(name: &str, kind: &str, objects: Vec<Object>) -> Layout {
    Layout { name: name.into(), kind: Arc::from(kind), objects, background: None, map: None, show_master: true, origin: None }
}

/// Office's layouts for a `w` by `h` slide.
pub fn office_layouts(w: f32, h: f32) -> Vec<Layout> {
    let m = w * 0.06875;
    let (cw, top, ch) = (w - 2.0 * m, h * 0.2662, h * 0.6345);
    let mut out = Vec::new();

    let mut ctr = placeholder(2, "Title 1", PhType::CtrTitle, 0, Some(Frame::new(w * 0.125, h * 0.1637, w * 0.75, h * 0.3482)));
    tune(&mut ctr, Some(60.0), Some(Align::Center), Some(VAnchor::Bottom), false);
    let mut sub = placeholder(3, "Subtitle 2", PhType::SubTitle, 1, Some(Frame::new(w * 0.125, h * 0.5252, w * 0.75, h * 0.2415)));
    tune(&mut sub, Some(24.0), Some(Align::Center), None, false);
    let mut n = 3;
    let mut v = vec![ctr, sub];
    v.extend(hf(&mut n));
    out.push(layout("Title Slide", "title", v));

    let mut n = 3;
    let mut v = vec![placeholder(2, "Title 1", PhType::Title, 0, None), placeholder(3, "Content Placeholder 2", PhType::Obj, 1, None)];
    v.extend(hf(&mut n));
    out.push(layout("Title and Content", "obj", v));

    let mut t = placeholder(2, "Title 1", PhType::Title, 0, Some(Frame::new(m, h * 0.2493, cw, h * 0.4159)));
    tune(&mut t, Some(60.0), None, Some(VAnchor::Bottom), false);
    let mut b = placeholder(3, "Text Placeholder 2", PhType::Body, 1, Some(Frame::new(m, h * 0.6692, cw, h * 0.2188)));
    tune(&mut b, Some(24.0), None, None, false);
    let mut n = 3;
    let mut v = vec![t, b];
    v.extend(hf(&mut n));
    out.push(layout("Section Header", "secHead", v));

    let half = (cw - 0.03 * w) / 2.0;
    let mut n = 4;
    let mut v = vec![
        placeholder(2, "Title 1", PhType::Title, 0, None),
        placeholder(3, "Content Placeholder 2", PhType::Obj, 1, Some(Frame::new(m, top, half, ch))),
        placeholder(4, "Content Placeholder 3", PhType::Obj, 2, Some(Frame::new(w - m - half, top, half, ch))),
    ];
    v.extend(hf(&mut n));
    out.push(layout("Two Content", "twoObj", v));

    let mut n = 2;
    let mut v = vec![placeholder(2, "Title 1", PhType::Title, 0, None)];
    v.extend(hf(&mut n));
    out.push(layout("Title Only", "titleOnly", v));

    let mut n = 1;
    out.push(layout("Blank", "blank", hf(&mut n)));

    let mut t = placeholder(2, "Title 1", PhType::Title, 0, Some(Frame::new(m, h * 0.0667, w * 0.35, h * 0.2333)));
    tune(&mut t, Some(32.0), None, Some(VAnchor::Bottom), false);
    let c = placeholder(3, "Content Placeholder 2", PhType::Obj, 1, Some(Frame::new(w * 0.45, h * 0.144, w * 0.48, h * 0.7106)));
    let mut b = placeholder(4, "Text Placeholder 3", PhType::Body, 2, Some(Frame::new(m, h * 0.3, w * 0.35, h * 0.5546)));
    tune(&mut b, Some(16.0), None, None, false);
    let mut n = 4;
    let mut v = vec![t, c, b];
    v.extend(hf(&mut n));
    out.push(layout("Content with Caption", "objTx", v));

    let mut t = placeholder(2, "Title 1", PhType::Title, 0, Some(Frame::new(m, h * 0.0667, w * 0.35, h * 0.2333)));
    tune(&mut t, Some(32.0), None, Some(VAnchor::Bottom), false);
    let p = placeholder(3, "Picture Placeholder 2", PhType::Pic, 1, Some(Frame::new(w * 0.45, h * 0.144, w * 0.48, h * 0.7106)));
    let mut b = placeholder(4, "Text Placeholder 3", PhType::Body, 2, Some(Frame::new(m, h * 0.3, w * 0.35, h * 0.5546)));
    tune(&mut b, Some(16.0), None, None, false);
    let mut n = 4;
    let mut v = vec![t, p, b];
    v.extend(hf(&mut n));
    out.push(layout("Picture with Caption", "picTx", v));
    out
}

/// A master in built-in theme `theme` for a `w` by `h` slide.
pub fn new_master(theme: usize, w: f32, h: f32) -> Master {
    let (title_style, body_style, other_style) = office_styles();
    Master {
        name: "Office Theme".into(),
        theme: SlideTheme { theme: Theme::builtin(theme), fmt: FormatScheme::office(), origin: None },
        map: ClrMap::default(),
        background: Some(Background::Ref(1001, SColor::scheme(Scheme::Bg1))),
        objects: master_objects(w, h),
        title_style,
        body_style,
        other_style,
        layouts: office_layouts(w, h),
        origin: None,
    }
}

/// A new presentation in built-in theme `theme`, with a title slide.
pub fn new_deck(theme: usize) -> Deck {
    let (w, h) = WIDE;
    let mut deck = Deck {
        width: w,
        height: h,
        notes_size: (540.0, 720.0),
        slides: Vec::new(),
        masters: vec![Arc::new(new_master(theme, w, h))],
        sections: Vec::new(),
        default_style: ListStyle::default(),
        show: ShowSettings::default(),
        first_number: 1,
        meta: apex_doc::Meta::default(),
        macros: apex_doc::Macros::default(),
        package: Arc::new(Package::default()),
        hf: HeaderFooter::default(),
    };
    crate::edit::add_slide(&mut deck, 0, 0, 0);
    deck
}
