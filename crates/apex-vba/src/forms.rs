//! The designs of UserForms: the controls of a form as the file keeps
//! them (MS-OFORMS), read from the streams of the form's storage in the
//! VBA project (`f`: the form and where its controls go, `o`: each
//! control's data, `i<ID>/…`: a Frame's or a MultiPage's own). Sizes are
//! in points. What isn't understood is left at its default; a damaged
//! design gives fewer controls, never an error.

use std::collections::HashMap;

/// The kinds of controls a form holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Label,
    TextBox,
    ComboBox,
    ListBox,
    CheckBox,
    OptionButton,
    ToggleButton,
    CommandButton,
    Frame,
    MultiPage,
    /// A page of a MultiPage.
    Page,
    TabStrip,
    Image,
    SpinButton,
    ScrollBar,
    /// Another ActiveX control: shown as an empty box.
    Other,
}

impl Kind {
    /// Its name in VBA (TypeName).
    pub fn type_name(self) -> &'static str {
        match self {
            Kind::Label => "Label",
            Kind::TextBox => "TextBox",
            Kind::ComboBox => "ComboBox",
            Kind::ListBox => "ListBox",
            Kind::CheckBox => "CheckBox",
            Kind::OptionButton => "OptionButton",
            Kind::ToggleButton => "ToggleButton",
            Kind::CommandButton => "CommandButton",
            Kind::Frame => "Frame",
            Kind::MultiPage => "MultiPage",
            Kind::Page => "Page",
            Kind::TabStrip => "TabStrip",
            Kind::Image => "Image",
            Kind::SpinButton => "SpinButton",
            Kind::ScrollBar => "ScrollBar",
            Kind::Other => "Control",
        }
    }

    /// The kind `Controls.Add "Forms.TextBox.1"` makes.
    pub fn from_prog_id(id: &str) -> Option<Kind> {
        let id = id.trim().to_ascii_lowercase();
        let name = id.strip_prefix("forms.")?.split('.').next()?;
        Some(match name {
            "label" => Kind::Label,
            "textbox" => Kind::TextBox,
            "combobox" => Kind::ComboBox,
            "listbox" => Kind::ListBox,
            "checkbox" => Kind::CheckBox,
            "optionbutton" => Kind::OptionButton,
            "togglebutton" => Kind::ToggleButton,
            "commandbutton" => Kind::CommandButton,
            "frame" => Kind::Frame,
            "multipage" => Kind::MultiPage,
            "tabstrip" => Kind::TabStrip,
            "image" => Kind::Image,
            "spinbutton" => Kind::SpinButton,
            "scrollbar" => Kind::ScrollBar,
            _ => return None,
        })
    }

    /// Whether it holds other controls.
    pub fn is_container(self) -> bool {
        matches!(self, Kind::Frame | Kind::MultiPage | Kind::Page)
    }
}

/// A control of a form, as designed.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub name: String,
    pub kind: Kind,
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub caption: String,
    /// Text, or the state of a CheckBox ("1", "0", "" for Null).
    pub value: String,
    pub tag: String,
    pub tip: String,
    /// OptionButtons with the same GroupName go together.
    pub group: String,
    /// A cell it shows and changes ("Sheet1!A1").
    pub control_source: String,
    /// The cells of a list's items ("Sheet1!A1:A10").
    pub row_source: String,
    pub visible: bool,
    pub enabled: bool,
    pub locked: bool,
    pub multiline: bool,
    pub word_wrap: bool,
    pub tab_index: i32,
    pub tab_stop: bool,
    /// The button Enter presses (Default) or Esc presses (Cancel).
    pub default: bool,
    pub cancel: bool,
    pub min: i32,
    pub max: i32,
    pub small_change: i32,
    pub large_change: i32,
    pub max_length: i32,
    pub password_char: String,
    pub column_count: i32,
    /// fmMultiSelect: 0 single, 1 multi, 2 extended.
    pub multi_select: u8,
    /// A ComboBox that only picks from its list (fmStyleDropDownList).
    pub drop_down_list: bool,
    /// RGB colors (`None`: the usual one).
    pub fore: Option<u32>,
    pub back: Option<u32>,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    /// The image of an Image or a button, as stored (BMP, GIF, JPEG...).
    pub picture: Option<std::sync::Arc<Vec<u8>>>,
    /// A Frame's controls, a MultiPage's pages, a Page's controls.
    pub children: Vec<Control>,
    /// Its own record as the file has it: written back as it was while
    /// what it shows is unchanged, so what isn't read here survives.
    pub raw: Option<std::sync::Arc<Raw>>,
}

/// A control's record as read, and what it showed then ([`Control::key`]).
#[derive(Debug, PartialEq)]
pub struct Raw {
    pub key: u64,
    pub bytes: Vec<u8>,
}

impl Control {
    pub fn new(kind: Kind, name: &str) -> Control {
        let (width, height) = match kind {
            Kind::Label => (72.0, 18.0),
            Kind::TextBox | Kind::ComboBox => (72.0, 18.0),
            Kind::ListBox => (72.0, 72.0),
            Kind::CheckBox | Kind::OptionButton => (108.0, 18.0),
            Kind::CommandButton | Kind::ToggleButton => (72.0, 24.0),
            Kind::SpinButton => (12.75, 25.5),
            Kind::ScrollBar => (12.75, 63.75),
            _ => (120.0, 90.0),
        };
        Control {
            name: name.to_string(),
            kind,
            left: 0.0,
            top: 0.0,
            width,
            height,
            caption: String::new(),
            value: String::new(),
            tag: String::new(),
            tip: String::new(),
            group: String::new(),
            control_source: String::new(),
            row_source: String::new(),
            visible: true,
            enabled: true,
            locked: false,
            multiline: false,
            word_wrap: matches!(kind, Kind::Label | Kind::TextBox | Kind::ComboBox | Kind::ListBox | Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton),
            tab_index: 0,
            tab_stop: !matches!(kind, Kind::Label | Kind::Image | Kind::Frame | Kind::Page),
            default: false,
            cancel: false,
            min: 0,
            max: if kind == Kind::ScrollBar { 32767 } else { 100 },
            small_change: 1,
            large_change: 1,
            max_length: 0,
            password_char: String::new(),
            column_count: 1,
            multi_select: 0,
            drop_down_list: false,
            fore: None,
            back: None,
            font_size: 8.25,
            bold: false,
            italic: false,
            picture: None,
            children: Vec::new(),
            raw: None,
        }
    }

    /// What of it its own record keeps (not where it goes, its name...):
    /// when this is as read, the record is written back as it was.
    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.kind.hash(&mut h);
        for v in [self.width, self.height, self.font_size] {
            v.to_bits().hash(&mut h);
        }
        (&self.caption, &self.value, &self.group, &self.password_char).hash(&mut h);
        (self.enabled, self.locked, self.multiline, self.word_wrap, self.bold, self.italic, self.drop_down_list).hash(&mut h);
        (self.min, self.max, self.small_change, self.large_change, self.max_length, self.column_count, self.multi_select).hash(&mut h);
        (self.fore, self.back, self.picture.as_deref()).hash(&mut h);
        h.finish()
    }

    /// Every control in it, at any depth, before its own children.
    pub fn walk<'a>(&'a self, out: &mut Vec<&'a Control>) {
        for c in &self.children {
            out.push(c);
            c.walk(out);
        }
    }
}

/// A form as designed.
#[derive(Clone, Debug, PartialEq)]
pub struct Design {
    pub caption: String,
    /// The inside of the form, in points.
    pub width: f32,
    pub height: f32,
    pub back: Option<u32>,
    /// StartUpPosition: 0 manual, 1 centered on the window, 2 on the screen.
    pub start_up: i32,
    pub left: f32,
    pub top: f32,
    pub controls: Vec<Control>,
}

impl Default for Design {
    fn default() -> Self {
        Design { caption: String::new(), width: 240.0, height: 180.0, back: None, start_up: 1, left: 0.0, top: 0.0, controls: Vec::new() }
    }
}

/// Controls a design may have in all (a form with more is cut short).
const MAX_CONTROLS: usize = 2000;
/// Frames inside frames.
const MAX_DEPTH: usize = 8;
/// The biggest picture kept.
const MAX_PICTURE: usize = 8 << 20;

fn himetric(v: i32) -> f32 {
    v as f32 * 72.0 / 2540.0
}

/// An OLE_COLOR as RGB, or None for a system color.
fn rgb(c: Option<u32>) -> Option<u32> {
    let c = c?;
    if c & 0xFF00_0000 != 0 {
        return None;
    }
    let (r, g, b) = (c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF);
    Some((r << 16) | (g << 8) | b)
}

/// Reads a stream of a design. Every read is checked: past the end
/// gives zeros and marks it failed.
struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(d: &'a [u8]) -> Reader<'a> {
        Reader { d, pos: 0 }
    }

    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let b = self.d.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(b)
    }

    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.bytes(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.bytes(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(self.u32()? as u64 | (self.u32()? as u64) << 32)
    }

    /// Skips to a multiple of `n` bytes from `from`.
    fn align(&mut self, from: usize, n: usize) {
        let off = (self.pos - from) % n;
        if off != 0 {
            self.pos += n - off;
        }
    }

    /// A string of an ExtraDataBlock: `count` with its compression flag,
    /// then padded to 4 bytes.
    fn string(&mut self, count: u32, from: usize) -> Option<String> {
        let compressed = count & 0x8000_0000 != 0;
        let n = (count & 0x7FFF_FFFF) as usize;
        let b = self.bytes(n)?;
        self.align(from, 4);
        Some(if compressed {
            b.iter().map(|&c| c as char).collect()
        } else {
            let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&units)
        })
    }
}

/// The fields of a DataBlock that a property mask says are there, in
/// order: (bit, size). Each is aligned to its size.
fn data_block(r: &mut Reader, mask: u64, fields: &[(u8, u8)]) -> Option<HashMap<u8, u32>> {
    let start = r.pos;
    let mut out = HashMap::new();
    for &(bit, size) in fields {
        if mask & (1 << bit) == 0 {
            continue;
        }
        r.align(start, size as usize);
        let v = match size {
            1 => r.u8()? as u32,
            2 => r.u16()? as u32,
            _ => r.u32()?,
        };
        out.insert(bit, v);
    }
    r.align(start, 4);
    Some(out)
}

/// A size (fmSize) or position: two 32-bit HIMETRIC values.
fn pair(r: &mut Reader) -> Option<(f32, f32)> {
    let a = r.u32()? as i32;
    let b = r.u32()? as i32;
    Some((himetric(a), himetric(b)))
}

/// Text properties (TextProps): font size, bold, italic.
fn text_props(r: &mut Reader, c: &mut Control) -> Option<()> {
    let (_minor, _major) = (r.u8()?, r.u8()?);
    let cb = r.u16()? as usize;
    let end = r.pos + cb;
    let mut b = Reader::new(r.bytes(cb)?);
    let mask = b.u32()? as u64;
    let start = b.pos;
    let f = data_block(&mut b, mask, &[(0, 4), (1, 4), (2, 4), (4, 1), (5, 1), (6, 1), (7, 2)])?;
    if let Some(&n) = f.get(&0) {
        b.string(n, start);
    }
    if let Some(&e) = f.get(&1) {
        c.bold = e & 1 != 0;
        c.italic = e & 2 != 0;
    }
    if let Some(&w) = f.get(&7) {
        c.bold |= w >= 600;
    }
    if let Some(&h) = f.get(&2).filter(|&&h| (20..=20 * 400).contains(&h)) {
        c.font_size = h as f32 / 20.0;
    }
    r.pos = end;
    Some(())
}

/// GuidAndPicture: the picture's bytes.
fn picture(r: &mut Reader) -> Option<Vec<u8>> {
    r.bytes(16)?;
    if r.u32()? != 0x0000_746C {
        return None;
    }
    let n = r.u32()? as usize;
    let b = r.bytes(n)?;
    (n <= MAX_PICTURE).then(|| b.to_vec())
}

/// GuidAndFont: skipped.
fn font(r: &mut Reader) -> Option<()> {
    let guid = r.bytes(16)?;
    if guid[..4] == [0x03, 0x52, 0xE3, 0x0B] {
        // StdFont: version, charset, flags, weight, height, name.
        r.bytes(10)?;
        let n = r.u8()? as usize;
        r.bytes(n)?;
    } else {
        let (_minor, _major) = (r.u8()?, r.u8()?);
        let cb = r.u16()? as usize;
        r.bytes(cb)?;
    }
    Some(())
}

/// The bits common to most controls (VariousPropertyBits), or the
/// kind's default ones the file leaves out.
fn various(c: &mut Control, v: Option<u32>, default: u32) {
    let v = v.unwrap_or(default);
    c.enabled = v & (1 << 1) != 0;
    c.locked = v & (1 << 2) != 0;
    c.word_wrap = v & (1 << 23) != 0;
    c.multiline = v & (1 << 31) != 0;
}

/// The VariousPropertyBits a record without them means.
pub(crate) const MORPH_BITS: u32 = 0x2C80_081B;
pub(crate) const LABEL_BITS: u32 = 0x0080_001B;
pub(crate) const PLAIN_BITS: u32 = 0x0000_001B;

/// Starts a control's record: versions, size; the record and its mask.
fn record<'a>(r: &mut Reader<'a>, wide_mask: bool) -> Option<(Reader<'a>, u64)> {
    let (_minor, _major) = (r.u8()?, r.u8()?);
    let cb = r.u16()? as usize;
    let mut b = Reader::new(r.bytes(cb)?);
    let mask = if wide_mask { b.u64()? } else { b.u32()? as u64 };
    Some((b, mask))
}

fn morph_data(r: &mut Reader, c: &mut Control) -> Option<()> {
    let (mut b, mask) = record(r, true)?;
    let start = b.pos;
    let f = data_block(
        &mut b,
        mask,
        &[(0, 4), (1, 4), (2, 4), (3, 4), (4, 1), (5, 1), (6, 1), (7, 1), (9, 2), (10, 4), (11, 2), (12, 2), (13, 2), (14, 2), (15, 2), (16, 1), (17, 1), (18, 1), (20, 1), (21, 1), (22, 4), (23, 4), (24, 4), (25, 4), (26, 4), (27, 2), (28, 2), (29, 2), (32, 4)],
    )?;
    various(c, f.get(&0).copied(), MORPH_BITS);
    c.back = rgb(f.get(&1).copied());
    c.fore = rgb(f.get(&2).copied());
    if let Some(&m) = f.get(&3) {
        c.max_length = m as i32;
    }
    if let Some(&p) = f.get(&9).filter(|&&p| p != 0) {
        c.password_char = char::from_u32(p).map(String::from).unwrap_or_default();
    }
    if let Some(&n) = f.get(&13) {
        c.column_count = (n as i16 as i32).clamp(-1, 64);
    }
    if let Some(&m) = f.get(&21) {
        c.multi_select = (m as u8).min(2);
    }
    // DisplayStyle: what the generic control is.
    match f.get(&6).copied().unwrap_or(1) {
        2 => c.kind = Kind::ListBox,
        3 => c.kind = Kind::ComboBox,
        4 => c.kind = Kind::CheckBox,
        5 => c.kind = Kind::OptionButton,
        6 => c.kind = Kind::ToggleButton,
        7 => {
            c.kind = Kind::ComboBox;
            c.drop_down_list = true;
        }
        _ if c.kind == Kind::Other => c.kind = Kind::TextBox,
        _ => {}
    }
    let from = start;
    if mask & (1 << 8) != 0 {
        let (w, h) = pair(&mut b)?;
        c.width = w;
        c.height = h;
    }
    if let Some(&n) = f.get(&22) {
        c.value = b.string(n, from)?;
    }
    if let Some(&n) = f.get(&23) {
        c.caption = b.string(n, from)?;
    }
    if let Some(&n) = f.get(&32) {
        c.group = b.string(n, from)?;
    }
    if mask & (1 << 27) != 0 {
        picture(r)?;
    }
    if mask & (1 << 28) != 0 {
        c.picture = picture(r).map(std::sync::Arc::new);
    }
    text_props(r, c)
}

/// A Label or CommandButton: their records differ in the middle.
fn captioned(r: &mut Reader, c: &mut Control, label: bool) -> Option<()> {
    let (mut b, mask) = record(r, false)?;
    let start = b.pos;
    let fields: &[(u8, u8)] = if label { &[(0, 4), (1, 4), (2, 4), (3, 4), (4, 4), (6, 1), (7, 4), (8, 2), (9, 2), (10, 2), (11, 2), (12, 2)] } else { &[(0, 4), (1, 4), (2, 4), (3, 4), (4, 4), (6, 1), (7, 2), (8, 2), (10, 2)] };
    let f = data_block(&mut b, mask, fields)?;
    c.fore = rgb(f.get(&0).copied());
    c.back = rgb(f.get(&1).copied());
    various(c, f.get(&2).copied(), if label { LABEL_BITS } else { PLAIN_BITS });
    if let Some(&n) = f.get(&3) {
        c.caption = b.string(n, start)?;
    }
    if mask & (1 << 5) != 0 {
        let (w, h) = pair(&mut b)?;
        c.width = w;
        c.height = h;
    }
    // Picture and MouseIcon, in the order each record keeps them.
    let (pic, icon) = if label { (10, 12) } else { (7, 10) };
    if mask & (1 << pic) != 0 {
        c.picture = picture(r).map(std::sync::Arc::new);
    }
    if mask & (1 << icon) != 0 {
        picture(r)?;
    }
    text_props(r, c)
}

/// SpinButton or ScrollBar: Min, Max, Value, the steps.
fn slider(r: &mut Reader, c: &mut Control, scroll: bool) -> Option<()> {
    let (mut b, mask) = record(r, false)?;
    let (fields, size_bit, small, large): (&[(u8, u8)], u8, u8, Option<u8>) = if scroll {
        (&[(0, 4), (1, 4), (2, 4), (4, 1), (5, 4), (6, 4), (7, 4), (9, 4), (10, 4), (11, 4), (12, 4), (13, 4), (14, 2), (15, 4), (16, 2)], 3, 11, Some(12))
    } else {
        (&[(0, 4), (1, 4), (2, 4), (5, 4), (6, 4), (7, 4), (8, 4), (9, 4), (10, 4), (11, 4), (12, 4), (13, 2), (14, 1)], 3, 10, None)
    };
    let f = data_block(&mut b, mask, fields)?;
    various(c, f.get(&2).copied(), PLAIN_BITS);
    if let Some(&v) = f.get(&5) {
        c.min = v as i32;
    }
    if let Some(&v) = f.get(&6) {
        c.max = v as i32;
    }
    if let Some(&v) = f.get(&7) {
        c.value = (v as i32).to_string();
    }
    if let Some(&v) = f.get(&small) {
        c.small_change = v as i32;
    }
    if let Some(&v) = large.and_then(|l| f.get(&l)) {
        c.large_change = v as i32;
    }
    if mask & (1 << size_bit) != 0 {
        let (w, h) = pair(&mut b)?;
        c.width = w;
        c.height = h;
    }
    Some(())
}

fn image(r: &mut Reader, c: &mut Control) -> Option<()> {
    let (mut b, mask) = record(r, false)?;
    let f = data_block(&mut b, mask, &[(3, 4), (4, 4), (5, 1), (6, 1), (7, 1), (8, 1), (10, 2), (11, 1), (13, 4), (14, 2)])?;
    c.back = rgb(f.get(&4).copied());
    various(c, f.get(&13).copied(), PLAIN_BITS);
    if mask & (1 << 9) != 0 {
        let (w, h) = pair(&mut b)?;
        c.width = w;
        c.height = h;
    }
    if mask & (1 << 10) != 0 {
        c.picture = picture(r).map(std::sync::Arc::new);
    }
    Some(())
}

/// A TabStrip's tab captions (a MultiPage's pages take them).
fn tab_strip(r: &mut Reader, c: &mut Control) -> Option<Vec<String>> {
    let (mut b, mask) = record(r, false)?;
    let start = b.pos;
    let f = data_block(&mut b, mask, &[(0, 4), (1, 4), (2, 4), (5, 4), (6, 1), (8, 4), (9, 4), (11, 4), (12, 4), (15, 4), (17, 4), (18, 4), (20, 4), (21, 4), (22, 4), (23, 4), (24, 2)])?;
    if let Some(&i) = f.get(&0) {
        c.value = (i as i32).to_string();
    }
    if mask & (1 << 4) != 0 {
        let (w, h) = pair(&mut b)?;
        c.width = w;
        c.height = h;
    }
    let mut items = Vec::new();
    if let Some(&n) = f.get(&5) {
        let end = b.pos.checked_add(n as usize)?;
        while b.pos < end && items.len() < 1000 {
            let count = b.u32()?;
            items.push(b.string(count, start)?);
        }
    }
    Some(items)
}

/// What a form, frame or page record (FormControl) says about itself.
#[derive(Default)]
struct FormHead {
    caption: Option<String>,
    size: Option<(f32, f32)>,
    back: Option<u32>,
}

/// A site: where a control goes, and which.
struct Site {
    control: Control,
    id: u32,
    class: u16,
    object_size: usize,
}

/// A FormControl record and its sites.
fn form_control(r: &mut Reader) -> Option<(FormHead, Vec<Site>)> {
    let (mut b, mask) = record(r, false)?;
    let start = b.pos;
    let f = data_block(&mut b, mask, &[(1, 4), (2, 4), (3, 4), (6, 4), (7, 1), (8, 1), (9, 1), (13, 4), (15, 2), (16, 1), (17, 1), (18, 4), (19, 4), (20, 2), (21, 2), (22, 4), (23, 1), (25, 1), (26, 4), (27, 4)])?;
    let mut head = FormHead { back: rgb(f.get(&1).copied()), ..Default::default() };
    if mask & (1 << 10) != 0 {
        head.size = pair(&mut b);
    }
    for bit in [11, 12] {
        if mask & (1 << bit) != 0 {
            pair(&mut b)?;
        }
    }
    if let Some(&n) = f.get(&19) {
        head.caption = b.string(n, start);
    }
    // FormStreamData.
    if mask & (1 << 15) != 0 {
        picture(r)?;
    }
    if mask & (1 << 20) != 0 {
        font(r)?;
    }
    if mask & (1 << 21) != 0 {
        picture(r)?;
    }
    // FormSiteData: the class table, unless a flag says it isn't saved.
    let flags = f.get(&6).copied().unwrap_or(0);
    if flags & (1 << 15) == 0 {
        let n = r.u16()?;
        for _ in 0..n {
            let (_v, cb) = (r.u16()?, r.u16()? as usize);
            r.bytes(cb)?;
        }
    }
    let count = r.u32()? as usize;
    let bytes = r.u32()? as usize;
    let mut s = Reader::new(r.bytes(bytes)?);
    // The depths and types of the sites, then aligned to 4.
    let mut left = count;
    while left > 0 {
        let _depth = s.u8()?;
        let mixed = s.u8()?;
        if mixed & 0x80 != 0 {
            s.u8()?;
            left = left.saturating_sub((mixed & 0x7F).max(1) as usize);
        } else {
            left -= 1;
        }
    }
    s.align(0, 4);
    let mut sites = Vec::new();
    for _ in 0..count.min(MAX_CONTROLS) {
        match site(&mut s) {
            Some(x) => sites.push(x),
            None => break,
        }
    }
    Some((head, sites))
}

fn site(r: &mut Reader) -> Option<Site> {
    let (mut b, mask) = record(r, false)?;
    let start = b.pos;
    let f = data_block(&mut b, mask, &[(0, 4), (1, 4), (2, 4), (3, 4), (4, 4), (5, 4), (6, 2), (7, 2), (9, 2), (11, 4), (12, 4), (13, 4), (14, 4)])?;
    let mut c = Control::new(Kind::Other, "");
    if let Some(&n) = f.get(&0) {
        c.name = b.string(n, start)?;
    }
    if let Some(&n) = f.get(&1) {
        c.tag = b.string(n, start)?;
    }
    if mask & (1 << 8) != 0 {
        let (l, t) = pair(&mut b)?;
        c.left = l;
        c.top = t;
    }
    if let Some(&n) = f.get(&11) {
        c.tip = b.string(n, start)?;
    }
    if let Some(&n) = f.get(&12) {
        b.string(n, start)?;
    }
    if let Some(&n) = f.get(&13) {
        c.control_source = b.string(n, start)?;
    }
    if let Some(&n) = f.get(&14) {
        c.row_source = b.string(n, start)?;
    }
    let bits = f.get(&4).copied().unwrap_or(0x33);
    c.tab_stop = bits & 1 != 0;
    c.visible = bits & 2 != 0;
    c.default = bits & 4 != 0;
    c.cancel = bits & 8 != 0;
    c.tab_index = f.get(&6).map_or(0, |&t| t as i16 as i32);
    let class = f.get(&7).map_or(0x7FFF, |&x| x as u16);
    c.kind = match class {
        7 => Kind::Page,
        12 => Kind::Image,
        14 => Kind::Frame,
        16 => Kind::SpinButton,
        17 => Kind::CommandButton,
        18 => Kind::TabStrip,
        21 => Kind::Label,
        23 => Kind::TextBox,
        24 => Kind::ListBox,
        25 => Kind::ComboBox,
        26 => Kind::CheckBox,
        27 => Kind::OptionButton,
        28 => Kind::ToggleButton,
        47 => Kind::ScrollBar,
        57 => Kind::MultiPage,
        _ => Kind::Other,
    };
    // A page's own flag says which one shows; the MultiPage's Value does.
    if c.kind == Kind::Page {
        c.visible = true;
    }
    if matches!(c.kind, Kind::SpinButton | Kind::ScrollBar) {
        c.value = "0".into();
    }
    let defaults = Control::new(c.kind, "");
    c.width = defaults.width;
    c.height = defaults.height;
    c.word_wrap = defaults.word_wrap;
    c.max = defaults.max;
    Some(Site { control: c, id: f.get(&2).copied().unwrap_or(0), class, object_size: f.get(&5).copied().unwrap_or(0) as usize })
}

/// The streams of a form's storage, by their path inside it ("f", "o",
/// "i08/f"...).
pub type Streams = HashMap<String, Vec<u8>>;

/// Reads the controls of the storage at `dir` ("" the form, "i08/" a
/// frame's): its head and its controls.
fn container(streams: &Streams, dir: &str, depth: usize, budget: &mut usize) -> Option<(FormHead, Vec<Control>)> {
    let f = streams.get(&format!("{dir}f"))?;
    let (head, sites) = form_control(&mut Reader::new(f))?;
    let empty = Vec::new();
    let o = streams.get(&format!("{dir}o")).unwrap_or(&empty);
    let mut data = Reader::new(o);
    let mut out = Vec::new();
    let mut tabs: Vec<String> = Vec::new();
    for s in sites {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let mut c = s.control;
        let Some(bytes) = data.bytes(s.object_size) else { break };
        let mut r = Reader::new(bytes);
        match s.class {
            12 => drop(image(&mut r, &mut c)),
            15 | 23..=28 => drop(morph_data(&mut r, &mut c)),
            16 => drop(slider(&mut r, &mut c, false)),
            17 => drop(captioned(&mut r, &mut c, false)),
            18 => tabs = tab_strip(&mut r, &mut c).unwrap_or_default(),
            21 => drop(captioned(&mut r, &mut c, true)),
            47 => drop(slider(&mut r, &mut c, true)),
            _ => {}
        }
        if !bytes.is_empty() {
            c.raw = Some(std::sync::Arc::new(Raw { key: c.key(), bytes: bytes.to_vec() }));
        }
        if c.kind.is_container() && depth < MAX_DEPTH {
            let sub = format!("{dir}i{:02}/", s.id);
            if let Some((h, children)) = container(streams, &sub, depth + 1, budget) {
                if let Some((w, hh)) = h.size {
                    c.width = w;
                    c.height = hh;
                }
                if let Some(cap) = h.caption {
                    c.caption = cap;
                }
                c.back = h.back;
                c.children = children;
            }
        }
        // A MultiPage's TabStrip only names its pages.
        if !(s.class == 18 && dir.starts_with('i') && streams.contains_key(&format!("{dir}x"))) {
            out.push(c);
        }
    }
    // A MultiPage's pages take their captions from its tabs.
    let mut pages = out.iter_mut().filter(|c| c.kind == Kind::Page);
    for t in tabs.iter() {
        match pages.next() {
            Some(p) if p.caption.is_empty() => p.caption = t.clone(),
            _ => {}
        }
    }
    Some((head, out))
}

/// A value of the `VBFrame` stream (`Caption = "Hello"`).
fn frame_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        if !k.trim().eq_ignore_ascii_case(key) {
            return None;
        }
        let v = v.trim();
        let v = match v.strip_prefix('"') {
            Some(q) => q.rsplit_once('"').map_or(q, |x| x.0).replace("\"\"", "\""),
            None => v.split('\'').next().unwrap_or("").trim().to_string(),
        };
        Some(v)
    })
}

impl Design {
    /// The design kept in a form's storage.
    pub fn read(streams: &Streams) -> Design {
        let mut d = Design::default();
        let frame = streams.get("\u{3}VBFrame").map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
        let mut budget = MAX_CONTROLS;
        if let Some((head, controls)) = container(streams, "", 0, &mut budget) {
            if let Some((w, h)) = head.size {
                d.width = w;
                d.height = h;
            }
            d.back = head.back;
            d.controls = controls;
        }
        if let Some(c) = frame_value(&frame, "Caption") {
            d.caption = c;
        }
        let twips = |k: &str| frame_value(&frame, k).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).map(|v| v / 20.0);
        if d.controls.is_empty() && !streams.contains_key("f") {
            if let (Some(w), Some(h)) = (twips("ClientWidth"), twips("ClientHeight")) {
                d.width = w;
                d.height = h;
            }
        }
        d.left = twips("ClientLeft").unwrap_or(0.0);
        d.top = twips("ClientTop").unwrap_or(0.0);
        if let Some(s) = frame_value(&frame, "StartUpPosition").and_then(|v| v.parse().ok()) {
            d.start_up = s;
        }
        d.width = d.width.clamp(20.0, 4000.0);
        d.height = d.height.clamp(20.0, 4000.0);
        d
    }

    /// Every control, at any depth.
    pub fn all(&self) -> Vec<&Control> {
        let mut out = Vec::new();
        for c in &self.controls {
            out.push(c);
            c.walk(&mut out);
        }
        out
    }
}

/// A control of a form while it runs: as designed (without its
/// children, which follow it with it as their parent), its list and what
/// is picked in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub c: Control,
    pub parent: Option<usize>,
    /// The rows of a ListBox or ComboBox, each with its columns.
    pub list: Vec<Vec<String>>,
    pub list_index: i32,
    /// The rows picked in a ListBox of several.
    pub selected: Vec<bool>,
    /// Removed with Controls.Remove.
    pub gone: bool,
    /// Changed since it got the focus (for BeforeUpdate and AfterUpdate).
    pub dirty: bool,
}

/// Which events of a control the form's code handles, so the window
/// sends them as they happen (else with the next one it sends).
pub mod handles {
    pub const CHANGE: u8 = 1;
    pub const KEY: u8 = 2;
    pub const FOCUS: u8 = 4;
    pub const CLICK: u8 = 8;
    pub const DBLCLICK: u8 = 16;
}

/// A form shown: what a window draws.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    /// Tells forms apart in [`Event::form`].
    pub id: u64,
    /// The form module's name.
    pub name: String,
    pub caption: String,
    pub width: f32,
    pub height: f32,
    pub left: f32,
    pub top: f32,
    pub start_up: i32,
    pub back: Option<u32>,
    pub items: Vec<Item>,
    /// The control with the focus.
    pub focus: Option<usize>,
    /// What the code handles, per item ([`handles`]).
    pub handles: Vec<u8>,
    /// Shown, and (false) under a modal form shown over it.
    pub shown: bool,
    pub enabled: bool,
    pub modal: bool,
    /// Unloaded: its controls are gone.
    pub unloaded: bool,
}

impl View {
    pub fn new(id: u64, name: &str, d: &Design) -> View {
        let mut items = Vec::new();
        fn add(c: &Control, parent: Option<usize>, items: &mut Vec<Item>) {
            let i = items.len();
            let mut own = c.clone();
            own.children = Vec::new();
            let list_index = if own.kind == Kind::MultiPage || own.kind == Kind::TabStrip { own.value.parse().unwrap_or(0) } else { -1 };
            items.push(Item { c: own, parent, list: Vec::new(), list_index, selected: Vec::new(), gone: false, dirty: false });
            for ch in &c.children {
                add(ch, Some(i), items);
            }
        }
        for c in &d.controls {
            add(c, None, &mut items);
        }
        let handles = vec![0; items.len()];
        View { id, name: name.to_string(), caption: d.caption.clone(), width: d.width, height: d.height, left: d.left, top: d.top, start_up: d.start_up, back: d.back, items, focus: None, handles, shown: false, enabled: true, modal: true, unloaded: false }
    }

    /// The control that takes the focus when the form shows: the first
    /// in the tab order (by its container's, then its own) that can.
    pub fn first_focus(&self) -> Option<usize> {
        let top = |mut i: usize| {
            let mut n = 0;
            while let Some(p) = self.items[i].parent {
                i = p;
                n += 1;
                if n > self.items.len() {
                    break;
                }
            }
            self.items[i].c.tab_index
        };
        let can = |i: usize| {
            let c = &self.items[i].c;
            !self.items[i].gone && c.tab_stop && c.visible && c.enabled && !matches!(c.kind, Kind::Label | Kind::Image | Kind::Frame | Kind::Page | Kind::MultiPage) && self.showing(i)
        };
        (0..self.items.len()).filter(|&i| can(i)).min_by_key(|&i| (top(i), self.items[i].c.tab_index, i))
    }

    /// The control named `name` (any case).
    pub fn find(&self, name: &str) -> Option<usize> {
        self.items.iter().position(|it| !it.gone && it.c.name.eq_ignore_ascii_case(name))
    }

    /// The controls directly in `parent` (None: on the form).
    pub fn children(&self, parent: Option<usize>) -> impl Iterator<Item = usize> + '_ {
        self.items.iter().enumerate().filter(move |(_, it)| !it.gone && it.parent == parent).map(|(i, _)| i)
    }

    /// The controls in `parent` at any depth (None: all).
    pub fn within(&self, parent: Option<usize>) -> Vec<usize> {
        (0..self.items.len()).filter(|&i| !self.items[i].gone && (parent.is_none() || self.is_inside(i, parent))).collect()
    }

    /// Whether `i` is inside `parent`, at any depth.
    pub fn is_inside(&self, i: usize, parent: Option<usize>) -> bool {
        let mut at = self.items.get(i).and_then(|it| it.parent);
        while let Some(p) = at {
            if Some(p) == parent {
                return true;
            }
            at = self.items.get(p).and_then(|it| it.parent);
        }
        parent.is_none()
    }

    /// The pages of MultiPage `i`.
    pub fn pages(&self, i: usize) -> Vec<usize> {
        self.children(Some(i)).filter(|&p| self.items[p].c.kind == Kind::Page).collect()
    }

    /// Whether control `i` shows: it and every container it is in are
    /// visible, and in a MultiPage it is on the page picked.
    pub fn showing(&self, i: usize) -> bool {
        let mut at = Some(i);
        while let Some(k) = at {
            let Some(it) = self.items.get(k) else { return false };
            if it.gone || !it.c.visible {
                return false;
            }
            if it.c.kind == Kind::Page
                && let Some(mp) = it.parent
                && self.pages(mp).get(self.items[mp].list_index.max(0) as usize) != Some(&k)
            {
                return false;
            }
            at = it.parent;
        }
        true
    }

    /// The text a list shows for row `r` (its TextColumn: the first).
    pub fn row_text(&self, i: usize, r: usize) -> String {
        self.items.get(i).and_then(|it| it.list.get(r)).and_then(|row| row.first()).cloned().unwrap_or_default()
    }
}

/// What changed in a control while the window had it.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// The text of a TextBox or ComboBox.
    Text(String),
    /// A CheckBox, OptionButton or ToggleButton.
    Checked(bool),
    /// The row picked in a list (-1: none), the page of a MultiPage or
    /// the tab of a TabStrip.
    Index(i32),
    /// The rows picked in a ListBox of several.
    Selected(Vec<bool>),
    /// A SpinButton or ScrollBar.
    Number(i32),
}

/// What the user did.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Nothing more than the changes.
    Changes,
    Click(usize),
    DblClick(usize),
    /// A key typed in a control, before it goes in (KeyPress).
    Key(usize, char),
    /// The focus went to a control.
    Focus(Option<usize>),
    /// The window's close button.
    Close,
}

/// What the window sends back.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub form: u64,
    pub changes: Vec<(usize, Change)>,
    pub action: Action,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_damaged_design_reads_as_what_there_is() {
        let mut s = Streams::new();
        s.insert("\u{3}VBFrame".into(), b"VERSION 5.00\r\nBegin {C62A69F0-16DC-11CE-9E98-00AA00574A4F} UserForm1\r\n   Caption = \"Hi \"\"there\"\"\"\r\n   ClientHeight = 3000\r\n   ClientWidth = 4000\r\nEnd\r\n".to_vec());
        let d = Design::read(&s);
        assert_eq!(d.caption, "Hi \"there\"");
        assert_eq!((d.width, d.height), (200.0, 150.0));
        for n in 0..200 {
            s.insert("f".into(), (0..n).map(|i| (i * 37 % 251) as u8).collect());
            s.insert("o".into(), vec![0xFF; n]);
            Design::read(&s);
        }
    }

    #[test]
    fn kinds_from_prog_ids() {
        assert_eq!(Kind::from_prog_id("Forms.TextBox.1"), Some(Kind::TextBox));
        assert_eq!(Kind::from_prog_id("forms.commandbutton.1"), Some(Kind::CommandButton));
        assert_eq!(Kind::from_prog_id("Excel.Application"), None);
    }
}
