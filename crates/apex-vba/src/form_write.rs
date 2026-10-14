//! Writes a UserForm's design as Excel keeps it (MS-OFORMS): the streams
//! of the form's storage, which [`Design::read`] reads back. Records are
//! laid out as Excel writes them; a control whose record was read and
//! whose own properties are unchanged keeps that record as it was.

use crate::forms::{Control, Design, Kind, Streams, LABEL_BITS, MORPH_BITS, PLAIN_BITS};

/// The class of a form, a page (storages of forms and pages).
const FORM_CLSID: [u8; 16] = [0xF0, 0x69, 0x2A, 0xC6, 0xDC, 0x16, 0xCE, 0x11, 0x9E, 0x98, 0x00, 0xAA, 0x00, 0x57, 0x4A, 0x4F];
const FRAME_CLSID: [u8; 16] = [0x20, 0x20, 0x18, 0x6E, 0x60, 0xF4, 0xCE, 0x11, 0x9B, 0xCD, 0x00, 0xAA, 0x00, 0x60, 0x8E, 0x01];
const MULTIPAGE_CLSID: [u8; 16] = [0x70, 0x13, 0xE3, 0x46, 0x7A, 0x3F, 0xCE, 0x11, 0xBE, 0xD6, 0x00, 0xAA, 0x00, 0x61, 0x10, 0x80];
/// StdFont and StdPicture.
const FONT_GUID: [u8; 16] = [0x03, 0x52, 0xE3, 0x0B, 0x91, 0x8F, 0xCE, 0x11, 0x9D, 0xE3, 0x00, 0xAA, 0x00, 0x4B, 0xB8, 0x51];
const PICTURE_GUID: [u8; 16] = [0x04, 0x52, 0xE3, 0x0B, 0x91, 0x8F, 0xCE, 0x11, 0x9D, 0xE3, 0x00, 0xAA, 0x00, 0x4B, 0xB8, 0x51];
/// The longest text kept in a record (a record holds at most 64 KB).
const MAX_TEXT: usize = 4096;

fn himetric(points: f32) -> u32 {
    let v = (points.clamp(-100_000.0, 100_000.0) * 2540.0 / 72.0).round();
    v as i32 as u32
}

/// RGB as an OLE_COLOR.
fn ole(rgb: u32) -> u32 {
    let (r, g, b) = ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF);
    r | (g << 8) | (b << 16)
}

/// A string of an ExtraDataBlock: its count (with the compression flag
/// when every character fits in a byte) and its bytes, padded to 4.
fn string(s: &str) -> (u32, Vec<u8>) {
    let s: String = s.chars().take(MAX_TEXT).collect();
    let mut b: Vec<u8>;
    let count = if s.chars().all(|c| (c as u32) < 0x100) {
        b = s.chars().map(|c| c as u8).collect();
        b.len() as u32 | 0x8000_0000
    } else {
        b = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
        b.len() as u32
    };
    while b.len() % 4 != 0 {
        b.push(0);
    }
    (count, b)
}

/// A record being built: the fields of its DataBlock and its
/// ExtraDataBlock, given in the order of their bits.
struct Rec {
    mask: u64,
    fields: Vec<(u8, u32)>,
    extra: Vec<u8>,
}

impl Rec {
    fn new() -> Rec {
        Rec { mask: 0, fields: Vec::new(), extra: Vec::new() }
    }

    /// A field of `size` bytes.
    fn field(&mut self, bit: u8, size: u8, v: u32) {
        self.mask |= 1 << bit;
        self.fields.push((size, v));
    }

    /// A property with no data (its bit says it).
    fn flag(&mut self, bit: u8) {
        self.mask |= 1 << bit;
    }

    /// A string: its count in the DataBlock, its text after it.
    fn text(&mut self, bit: u8, s: &str) {
        let (count, b) = string(s);
        self.field(bit, 4, count);
        self.extra.extend(b);
    }

    /// A size or position: two HIMETRIC values after the DataBlock.
    fn pair(&mut self, bit: u8, a: f32, b: f32) {
        self.mask |= 1 << bit;
        self.extra.extend(himetric(a).to_le_bytes());
        self.extra.extend(himetric(b).to_le_bytes());
    }

    /// The record: versions, size, mask (64 bits when `wide`), DataBlock
    /// (each field aligned to its size, the whole to 4), ExtraDataBlock.
    fn bytes(&self, major: u8, wide: bool) -> Vec<u8> {
        let mut block: Vec<u8> = Vec::new();
        for &(size, v) in &self.fields {
            while block.len() % size as usize != 0 {
                block.push(0);
            }
            block.extend(&v.to_le_bytes()[..size as usize]);
        }
        while block.len() % 4 != 0 {
            block.push(0);
        }
        let mut body = if wide { self.mask.to_le_bytes().to_vec() } else { (self.mask as u32).to_le_bytes().to_vec() };
        body.extend(block);
        body.extend(&self.extra);
        let mut out = vec![0, major];
        out.extend((body.len().min(0xFFFF) as u16).to_le_bytes());
        out.extend(body);
        out
    }
}

/// GuidAndPicture.
fn picture(out: &mut Vec<u8>, data: &[u8]) {
    out.extend(PICTURE_GUID);
    out.extend(0x746Cu32.to_le_bytes());
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
}

/// GuidAndFont: Tahoma 8.25, as Excel writes it.
fn std_font(out: &mut Vec<u8>) {
    out.extend(FONT_GUID);
    out.extend([1, 0, 0, 0, 0x90, 0x01]);
    out.extend(82_500u32.to_le_bytes());
    out.push(6);
    out.extend(b"Tahoma");
}

/// TextProps: the font of a control's text; `align`: ParagraphAlign.
fn text_props(out: &mut Vec<u8>, c: &Control, align: Option<u8>) {
    let mut r = Rec::new();
    let (count, name) = string("Tahoma");
    r.field(0, 4, count);
    let effects = c.bold as u32 | (c.italic as u32) << 1;
    if effects != 0 {
        r.field(1, 4, effects);
    }
    r.field(2, 4, (c.font_size.clamp(1.0, 400.0) * 20.0).round() as u32);
    r.field(4, 1, 0);
    r.field(5, 1, 2);
    if let Some(a) = align {
        r.field(6, 1, a as u32);
    }
    r.extra.extend(name);
    out.extend(r.bytes(2, false));
}

/// VariousPropertyBits: `base` with the control's own.
fn various(c: &Control, base: u32) -> u32 {
    let mut v = base;
    for (bit, on) in [(1, c.enabled), (2, c.locked), (23, c.word_wrap), (31, c.multiline)] {
        if on {
            v |= 1 << bit;
        } else {
            v &= !(1 << bit);
        }
    }
    v
}

fn colors(r: &mut Rec, c: &Control) {
    if let Some(x) = c.fore {
        r.field(0, 4, ole(x));
    }
    if let Some(x) = c.back {
        r.field(1, 4, ole(x));
    }
}

/// A TextBox, ComboBox, ListBox, CheckBox, OptionButton or ToggleButton
/// (MorphDataControl).
fn morph(c: &Control) -> Vec<u8> {
    let mut r = Rec::new();
    let editable = matches!(c.kind, Kind::TextBox | Kind::ComboBox);
    r.field(0, 4, various(c, if editable { MORPH_BITS | 0x4000 } else { MORPH_BITS }));
    // Buttons' colors (ButtonFace, ButtonText) unless their own.
    let button = matches!(c.kind, Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton);
    if let Some(x) = c.back.map(ole).or(button.then_some(0x8000_000F)) {
        r.field(1, 4, x);
    }
    if let Some(x) = c.fore.map(ole).or(button.then_some(0x8000_0012)) {
        r.field(2, 4, x);
    }
    if c.max_length > 0 && editable {
        r.field(3, 4, c.max_length as u32);
    }
    let style = match c.kind {
        Kind::ListBox => 2,
        Kind::ComboBox if c.drop_down_list => 7,
        Kind::ComboBox => 3,
        Kind::CheckBox => 4,
        Kind::OptionButton => 5,
        Kind::ToggleButton => 6,
        _ => 1,
    };
    if style != 1 {
        r.field(6, 1, style);
    }
    r.flag(8);
    let password = c.password_char.chars().next().filter(|_| c.kind == Kind::TextBox);
    if let Some(p) = password {
        r.field(9, 2, (p as u32).min(0xFFFF));
    }
    if matches!(c.kind, Kind::ListBox | Kind::ComboBox) && c.column_count != 1 {
        r.field(13, 2, c.column_count.clamp(-1, 64) as i16 as u16 as u32);
    }
    if c.kind == Kind::ComboBox {
        // Completes what is typed; its button always shows.
        r.field(16, 1, 1);
        r.field(18, 1, 2);
    }
    if c.kind == Kind::ListBox && c.multi_select > 0 {
        r.field(21, 1, c.multi_select.min(2) as u32);
    }
    // The size, then the texts, after the DataBlock, in the order of
    // their bits.
    let value = (!c.value.is_empty()).then(|| string(&c.value));
    if let Some((count, _)) = &value {
        r.field(22, 4, *count);
    }
    let captioned = matches!(c.kind, Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton);
    let caption = (captioned && !c.caption.is_empty()).then(|| string(&c.caption));
    if let Some((count, _)) = &caption {
        r.field(23, 4, *count);
    }
    if c.picture.is_some() {
        r.field(28, 2, 0xFFFF);
    }
    r.flag(31);
    let group = (c.kind == Kind::OptionButton && !c.group.is_empty()).then(|| string(&c.group));
    if let Some((count, _)) = &group {
        r.field(32, 4, *count);
    }
    r.extra.extend(himetric(c.width).to_le_bytes());
    r.extra.extend(himetric(c.height).to_le_bytes());
    for (_, b) in [value, caption, group].into_iter().flatten() {
        r.extra.extend(b);
    }
    let mut out = r.bytes(2, true);
    if let Some(p) = &c.picture {
        picture(&mut out, p);
    }
    text_props(&mut out, c, (c.kind == Kind::ToggleButton).then_some(3));
    out
}

/// A Label or a CommandButton.
fn captioned(c: &Control, label: bool) -> Vec<u8> {
    let mut r = Rec::new();
    colors(&mut r, c);
    let base = if label { LABEL_BITS } else { PLAIN_BITS };
    let v = various(c, base);
    if v != base {
        r.field(2, 4, v);
    }
    if !c.caption.is_empty() {
        r.text(3, &c.caption);
    }
    r.pair(5, c.width, c.height);
    if c.picture.is_some() {
        r.field(if label { 10 } else { 7 }, 2, 0xFFFF);
    }
    let mut out = r.bytes(2, false);
    if let Some(p) = &c.picture {
        picture(&mut out, p);
    }
    text_props(&mut out, c, (!label).then_some(3));
    out
}

/// A SpinButton or a ScrollBar.
fn slider(c: &Control) -> Vec<u8> {
    let scroll = c.kind == Kind::ScrollBar;
    let mut r = Rec::new();
    colors(&mut r, c);
    let v = various(c, PLAIN_BITS);
    if v != PLAIN_BITS {
        r.field(2, 4, v);
    }
    r.pair(3, c.width, c.height);
    if c.min != 0 {
        r.field(5, 4, c.min as u32);
    }
    if c.max != if scroll { 32767 } else { 100 } {
        r.field(6, 4, c.max as u32);
    }
    let value: i32 = c.value.trim().parse().unwrap_or(0);
    if value != 0 {
        r.field(7, 4, value as u32);
    }
    if c.small_change != 1 {
        r.field(if scroll { 11 } else { 10 }, 4, c.small_change as u32);
    }
    if scroll && c.large_change != 1 {
        r.field(12, 4, c.large_change as u32);
    }
    r.bytes(2, false)
}

fn image(c: &Control) -> Vec<u8> {
    let mut r = Rec::new();
    if let Some(x) = c.back {
        r.field(4, 4, ole(x));
    }
    r.pair(9, c.width, c.height);
    if c.picture.is_some() {
        r.field(10, 2, 0xFFFF);
    }
    let v = various(c, PLAIN_BITS);
    if v != PLAIN_BITS {
        r.field(13, 4, v);
    }
    let mut out = r.bytes(2, false);
    if let Some(p) = &c.picture {
        picture(&mut out, p);
    }
    out
}

/// Strings one after another, each with its count (a TabStrip's lists).
fn strings<'a>(items: impl Iterator<Item = &'a str>) -> Vec<u8> {
    let mut out = Vec::new();
    for t in items {
        let (count, b) = string(t);
        out.extend(count.to_le_bytes());
        out.extend(b);
    }
    out
}

/// A TabStrip with these tabs (a MultiPage's name its pages).
fn tab_strip(c: &Control, tabs: &[String], index: u32) -> Vec<u8> {
    let n = tabs.len() as u32;
    let items = strings(tabs.iter().map(String::as_str));
    let names: Vec<String> = (1..=n).map(|i| format!("Tab{i}")).collect();
    let names = strings(names.iter().map(String::as_str));
    let empty = vec![0u8; 4 * n as usize];
    let mut r = Rec::new();
    r.field(0, 4, index);
    r.pair(4, c.width, c.height);
    r.field(5, 4, items.len() as u32);
    r.field(15, 4, empty.len() as u32);
    r.field(17, 4, names.len() as u32);
    r.flag(19);
    r.field(20, 4, n + 1);
    r.field(21, 4, empty.len() as u32);
    r.field(22, 4, n);
    r.field(23, 4, empty.len() as u32);
    for part in [&items, &empty, &names, &empty, &empty] {
        r.extra.extend(part);
    }
    let mut out = r.bytes(2, false);
    text_props(&mut out, c, None);
    // Each tab: visible and enabled.
    for _ in 0..n {
        out.extend(3u32.to_le_bytes());
    }
    out
}

/// A CompObj stream: the class of its storage, its names.
fn comp_obj(clsid: Option<[u8; 16]>, user_type: &str, prog_id: &str) -> Vec<u8> {
    let mut out = vec![1, 0, 0xFE, 0xFF, 3, 0x0A, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF];
    out.extend(clsid.unwrap_or([0; 16]));
    for s in [user_type, "Embedded Object", prog_id] {
        if s.is_empty() {
            out.extend(0u32.to_le_bytes());
        } else {
            out.extend((s.len() as u32 + 1).to_le_bytes());
            out.extend(s.as_bytes());
            out.push(0);
        }
    }
    out.extend(0x71B2_39F4u32.to_le_bytes());
    out.extend([0; 12]);
    out
}

/// What a FormControl record (a form, frame, multipage or page) is.
enum Holder<'a> {
    Form(&'a Design),
    Frame(&'a Control),
    MultiPage(&'a Control),
    Page(&'a Control),
}

/// A site: a control of a container, where it goes.
struct Site<'a> {
    c: &'a Control,
    id: u32,
    class: u16,
    data: usize,
    picked: bool,
}

fn class_of(k: Kind) -> Option<u16> {
    Some(match k {
        Kind::Page => 7,
        Kind::Image => 12,
        Kind::Frame => 14,
        Kind::SpinButton => 16,
        Kind::CommandButton => 17,
        Kind::TabStrip => 18,
        Kind::Label => 21,
        Kind::TextBox => 23,
        Kind::ListBox => 24,
        Kind::ComboBox => 25,
        Kind::CheckBox => 26,
        Kind::OptionButton => 27,
        Kind::ToggleButton => 28,
        Kind::ScrollBar => 47,
        Kind::MultiPage => 57,
        Kind::Other => return None,
    })
}

fn site_record(s: &Site) -> Vec<u8> {
    let c = s.c;
    let mut r = Rec::new();
    if !c.name.is_empty() {
        r.text(0, &c.name);
    }
    if !c.tag.is_empty() {
        r.text(1, &c.tag);
    }
    r.field(2, 4, s.id);
    // A MultiPage's page shows when it is the one picked.
    let visible = if c.kind == Kind::Page { s.picked } else { c.visible };
    let mut bits = 0x20 | c.tab_stop as u32 | (visible as u32) << 1 | (c.default as u32) << 2 | (c.cancel as u32) << 3;
    if s.data > 0 {
        bits |= 0x10;
    }
    if c.kind.is_container() {
        bits |= 0x4_0000;
    }
    if bits != 0x33 {
        r.field(4, 4, bits);
    }
    if s.data > 0 {
        r.field(5, 4, s.data as u32);
    }
    r.field(6, 2, c.tab_index.clamp(-1, 0x7FFF) as i16 as u16 as u32);
    r.field(7, 2, s.class as u32);
    r.pair(8, c.left, c.top);
    for (bit, text) in [(11, &c.tip), (13, &c.control_source), (14, &c.row_source)] {
        if !text.is_empty() {
            r.text(bit, text);
        }
    }
    r.bytes(0, false)
}

struct Writer {
    out: Streams,
    next_id: u32,
    /// NextAvailableID: one past every id the form gives.
    total: u32,
}

impl Writer {
    /// A control's record in its container's `o` stream.
    fn object(&self, c: &Control) -> Vec<u8> {
        if let Some(raw) = c.raw.as_ref().filter(|r| r.key == c.key()) {
            return raw.bytes.clone();
        }
        match c.kind {
            Kind::Label => captioned(c, true),
            Kind::CommandButton => captioned(c, false),
            Kind::SpinButton | Kind::ScrollBar => slider(c),
            Kind::Image => image(c),
            Kind::TabStrip => tab_strip(c, &["Tab1".to_string(), "Tab2".to_string()], 0),
            Kind::TextBox | Kind::ComboBox | Kind::ListBox | Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton => morph(c),
            Kind::Frame | Kind::MultiPage | Kind::Page | Kind::Other => Vec::new(),
        }
    }

    /// The storage at `dir` of a form, frame or page: `f`, `o`, and the
    /// storages of the containers in it.
    fn container(&mut self, dir: &str, holder: Holder, depth: usize) {
        let controls: &[Control] = match &holder {
            Holder::Form(d) => &d.controls,
            Holder::Frame(c) | Holder::Page(c) => &c.children,
            Holder::MultiPage(_) => &[],
        };
        let mut sites = Vec::new();
        let mut o = Vec::new();
        for c in controls.iter().filter(|c| c.kind != Kind::Page) {
            let Some(class) = class_of(c.kind) else { continue };
            self.next_id += 1;
            let id = self.next_id;
            let data = self.object(c);
            o.extend(&data);
            sites.push(Site { c, id, class, data: data.len(), picked: false });
            let sub = format!("{dir}i{id:02}/");
            match c.kind {
                Kind::Frame if depth < 8 => {
                    self.out.insert(format!("{sub}\u{1}CompObj"), comp_obj(Some(FRAME_CLSID), "Microsoft Forms 2.0 Frame", "Forms.Frame.1"));
                    self.container(&sub, Holder::Frame(c), depth + 1);
                }
                Kind::MultiPage if depth < 8 => self.multi_page(&sub, c, depth + 1),
                _ => {}
            }
        }
        self.out.insert(format!("{dir}f"), self.form_stream(&holder, &sites));
        self.out.insert(format!("{dir}o"), o);
    }

    /// A MultiPage's storage: its TabStrip and its pages.
    fn multi_page(&mut self, dir: &str, mp: &Control, depth: usize) {
        let pages: Vec<&Control> = mp.children.iter().filter(|c| c.kind == Kind::Page).collect();
        self.next_id += 1;
        let tab_id = self.next_id;
        let tabs: Vec<String> = pages.iter().map(|p| p.caption.clone()).collect();
        let index = mp.value.trim().parse::<u32>().unwrap_or(0).min(tabs.len().saturating_sub(1) as u32);
        let mut strip = Control::new(Kind::TabStrip, "");
        strip.width = mp.width;
        strip.height = mp.height;
        let o = tab_strip(&strip, &tabs, index);
        let mut ids = Vec::new();
        for p in &pages {
            self.next_id += 1;
            let id = self.next_id;
            ids.push(id);
            let sub = format!("{dir}i{id:02}/");
            self.out.insert(format!("{sub}\u{1}CompObj"), comp_obj(Some(FORM_CLSID), "Microsoft Forms 2.0 Form", "Forms.Form.1"));
            self.container(&sub, Holder::Page(p), depth + 1);
        }
        let mut sites = vec![Site { c: &strip, id: tab_id, class: 18, data: o.len(), picked: false }];
        for (i, (p, &id)) in pages.iter().zip(&ids).enumerate() {
            sites.push(Site { c: p, id, class: 7, data: 0, picked: i as u32 == index });
        }
        // PageProperties for each site, then MultiPageProperties: the
        // pages, the TabStrip's id, the pages' ids.
        let mut x = Vec::new();
        for _ in &sites {
            x.extend([0, 2, 4, 0, 0, 0, 0, 0]);
        }
        x.extend([0, 2, 12, 0, 6, 0, 0, 0]);
        x.extend((pages.len() as u32).to_le_bytes());
        x.extend(tab_id.to_le_bytes());
        for id in ids {
            x.extend(id.to_le_bytes());
        }
        self.out.insert(format!("{dir}\u{1}CompObj"), comp_obj(Some(MULTIPAGE_CLSID), "Microsoft Forms 2.0 Form", "Forms.MultiPage.1"));
        self.out.insert(format!("{dir}f"), self.form_stream(&Holder::MultiPage(mp), &sites));
        self.out.insert(format!("{dir}o"), o);
        self.out.insert(format!("{dir}x"), x);
    }

    /// A FormControl record and its sites.
    fn form_stream(&self, holder: &Holder, sites: &[Site]) -> Vec<u8> {
        let (back, size, caption, flags, font) = match holder {
            Holder::Form(d) => (d.back, (d.width, d.height), None, None, true),
            Holder::Frame(c) => (c.back, (c.width, c.height), Some(c.caption.as_str()).filter(|s| !s.is_empty()), Some(0x8004), true),
            Holder::MultiPage(c) => (None, (c.width, c.height), None, Some(0xC004), false),
            Holder::Page(c) => (c.back, (c.width, c.height), None, Some(0x8004), false),
        };
        let mut r = Rec::new();
        if let Some(b) = back {
            r.field(1, 4, ole(b));
        }
        r.field(3, 4, self.total);
        if let Some(f) = flags {
            r.field(6, 4, f);
        }
        r.pair(10, size.0, size.1);
        r.pair(11, 0.0, 0.0);
        if matches!(holder, Holder::Frame(_)) {
            r.field(17, 1, 3);
        }
        if let Some(cap) = caption {
            r.text(19, cap);
        }
        if font {
            r.field(20, 2, 0xFFFF);
        }
        r.field(26, 4, self.total);
        r.field(27, 4, 32_000);
        let mut out = r.bytes(4, false);
        if font {
            std_font(&mut out);
        }
        // The class table: none (the flag says so), or empty.
        if flags.is_none() {
            out.extend([0, 0]);
        }
        let mut area = Vec::new();
        for _ in sites {
            area.extend([0, 1]);
        }
        while area.len() % 4 != 0 {
            area.push(0);
        }
        for s in sites {
            area.extend(site_record(s));
        }
        out.extend((sites.len() as u32).to_le_bytes());
        out.extend((area.len() as u32).to_le_bytes());
        out.extend(area);
        out
    }
}

/// A value of a `VBFrame` stream: quoted text, its quotes doubled.
fn quoted(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect();
    format!("\"{}\"", s.replace('"', "\"\""))
}

impl Design {
    /// The streams of the form's storage for this design, the form named
    /// `name` (`VBFrame` as UTF-8, as [`Design::read`] takes it). Other
    /// ActiveX controls aren't written.
    pub fn write(&self, name: &str) -> Streams {
        let mut count = 0u32;
        for c in self.all() {
            count += match c.kind {
                Kind::MultiPage => 2,
                Kind::Other => 0,
                _ => 1,
            };
        }
        let mut w = Writer { out: Streams::new(), next_id: 0, total: count + 1 };
        w.container("", Holder::Form(self), 0);
        let twips = |v: f32| (v.clamp(0.0, 100_000.0) * 20.0).round() as i64;
        let position = match self.start_up {
            0 => "0  'Manual",
            2 => "2  'CenterScreen",
            3 => "3  'Windows Default",
            _ => "1  'CenterOwner",
        };
        let frame = format!(
            "VERSION 5.00\r\nBegin {{C62A69F0-16DC-11CE-9E98-00AA00574A4F}} {name} \r\n   Caption         =   {}\r\n   ClientHeight    =   {}\r\n   ClientLeft      =   {}\r\n   ClientTop       =   {}\r\n   ClientWidth     =   {}\r\n   StartUpPosition =   {position}\r\nEnd\r\n",
            quoted(&self.caption),
            twips(self.height),
            twips(self.left),
            twips(self.top),
            twips(self.width),
        );
        w.out.insert("\u{3}VBFrame".into(), frame.into_bytes());
        w.out.insert("\u{1}CompObj".into(), comp_obj(None, "Microsoft Forms 2.0 Form", ""));
        w.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenth(v: f32) -> f32 {
        (v * 10.0).round() / 10.0
    }

    /// Without what reading adds (the records as read), sizes to a tenth
    /// of a point (HIMETRIC keeps them to about a thirtieth).
    fn bare(mut c: Control) -> Control {
        c.raw = None;
        for v in [&mut c.left, &mut c.top, &mut c.width, &mut c.height] {
            *v = tenth(*v);
        }
        c.children = c.children.into_iter().map(bare).collect();
        c
    }

    fn sample() -> Design {
        let mut d = Design { caption: "Pick \"one\"".into(), width: 300.0, height: 200.0, start_up: 0, left: 30.0, top: 40.0, ..Design::default() };
        let mut add = |kind, name: &str, f: &dyn Fn(&mut Control)| {
            let mut c = Control::new(kind, name);
            c.left = 6.0 * d.controls.len() as f32;
            c.top = 12.0;
            c.tab_index = d.controls.len() as i32;
            f(&mut c);
            d.controls.push(c);
        };
        add(Kind::Label, "Label1", &|c| {
            c.caption = "Name:".into();
            c.fore = Some(0x112233);
        });
        add(Kind::TextBox, "TextBox1", &|c| {
            c.value = "héllo ✓".into();
            c.multiline = true;
            c.max_length = 20;
            c.control_source = "Sheet1!A1".into();
            c.tip = "Type here".into();
        });
        add(Kind::TextBox, "Secret", &|c| c.password_char = "*".into());
        add(Kind::ComboBox, "ComboBox1", &|c| {
            c.drop_down_list = true;
            c.row_source = "Sheet1!B1:B5".into();
            c.column_count = 2;
        });
        add(Kind::ListBox, "ListBox1", &|c| c.multi_select = 1);
        add(Kind::CheckBox, "CheckBox1", &|c| {
            c.caption = "Yes".into();
            c.value = "1".into();
        });
        add(Kind::OptionButton, "OptionButton1", &|c| {
            c.caption = "A".into();
            c.group = "G".into();
        });
        add(Kind::ToggleButton, "ToggleButton1", &|c| {
            c.caption = "T".into();
            c.enabled = false;
        });
        add(Kind::CommandButton, "OK", &|c| {
            c.caption = "OK".into();
            c.default = true;
            c.bold = true;
            c.font_size = 10.0;
        });
        add(Kind::CommandButton, "Cancel", &|c| {
            c.caption = "Cancel".into();
            c.cancel = true;
            c.visible = false;
            c.tag = "x".into();
        });
        add(Kind::SpinButton, "SpinButton1", &|c| {
            c.min = -5;
            c.max = 50;
            c.value = "7".into();
            c.small_change = 2;
        });
        add(Kind::ScrollBar, "ScrollBar1", &|c| {
            c.value = "0".into();
            c.large_change = 10;
        });
        add(Kind::Image, "Image1", &|c| c.picture = Some(std::sync::Arc::new(b"GIF89a....".to_vec())));
        add(Kind::Frame, "Frame1", &|c| {
            c.caption = "Options".into();
            let mut t = Control::new(Kind::TextBox, "TextBox2");
            t.left = 3.0;
            c.children.push(t);
        });
        add(Kind::MultiPage, "MultiPage1", &|c| {
            for (i, cap) in ["First", "Second"].iter().enumerate() {
                let mut p = Control::new(Kind::Page, &format!("Page{}", i + 1));
                p.caption = cap.to_string();
                p.tab_index = i as i32;
                p.tab_stop = false;
                if i == 0 {
                    p.children.push(Control::new(Kind::CheckBox, "CheckBox2"));
                }
                c.children.push(p);
            }
        });
        d
    }

    #[test]
    fn a_design_reads_back_as_written() {
        let d = sample();
        let s = d.write("UserForm1");
        let back = Design::read(&s);
        let round = |d: &Design| Design { controls: d.controls.iter().cloned().map(bare).collect(), width: tenth(d.width), height: tenth(d.height), ..d.clone() };
        assert_eq!(round(&back), round(&d));
        // Written again, it is the same: each record kept as read.
        assert_eq!(back.write("UserForm1"), s);
        let mut moved = back.clone();
        moved.controls[1].value = "changed".into();
        let again = Design::read(&moved.write("UserForm1"));
        assert_eq!(again.controls[1].value, "changed");
        assert_eq!(again.controls[2].password_char, "*");
    }

    #[test]
    fn the_storages_are_laid_out_as_excel_lays_them() {
        let s = sample().write("UserForm1");
        let mut keys: Vec<&str> = s.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            ["\u{1}CompObj", "\u{3}VBFrame", "f", "i14/\u{1}CompObj", "i14/f", "i14/o", "i16/\u{1}CompObj", "i16/f", "i16/i18/\u{1}CompObj", "i16/i18/f", "i16/i18/o", "i16/i20/\u{1}CompObj", "i16/i20/f", "i16/i20/o", "i16/o", "i16/x", "o"]
        );
        let frame = String::from_utf8(s["\u{3}VBFrame"].clone()).unwrap();
        assert!(frame.contains("Begin {C62A69F0-16DC-11CE-9E98-00AA00574A4F} UserForm1 \r\n"), "{frame}");
        assert!(frame.contains("Caption         =   \"Pick \"\"one\"\"\""), "{frame}");
        assert!(frame.contains("ClientWidth     =   6000"), "{frame}");
        // A label's record as Excel writes one.
        let mut l = Control::new(Kind::Label, "Label1");
        l.caption = "Label1".into();
        l.width = himetric_back(0x1304);
        l.height = himetric_back(0x771);
        let o = captioned(&l, true);
        assert_eq!(&o[..0x1C], &[0, 2, 0x18, 0, 0x28, 0, 0, 0, 6, 0, 0, 0x80, b'L', b'a', b'b', b'e', b'l', b'1', 0, 0, 0x04, 0x13, 0, 0, 0x71, 7, 0, 0]);
        // A form's CompObj as Excel writes it.
        assert_eq!(s["\u{1}CompObj"].len(), 97);
        assert_eq!(s["i14/\u{1}CompObj"].len(), 112);
    }

    fn himetric_back(v: u32) -> f32 {
        v as f32 * 72.0 / 2540.0
    }
}
