//! A canvas: a page where things go anywhere. Cards of other pages,
//! notes, pictures and groups, joined by arrows; an arrow between two
//! page cards is a link between those pages, with what it means.

use std::sync::Arc;

use super::Rel;

/// What a page is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageKind {
    #[default]
    Text,
    Canvas,
}

impl PageKind {
    pub fn key(self) -> &'static str {
        match self {
            PageKind::Text => "text",
            PageKind::Canvas => "canvas",
        }
    }

    pub fn from_key(s: &str) -> PageKind {
        if s == "canvas" { PageKind::Canvas } else { PageKind::Text }
    }
}

/// Colors offered for notes and groups (RGB).
pub const NOTE_COLORS: [u32; 6] = [0xFFF2A8, 0xC9F0C4, 0xCFE3FF, 0xFFD1DC, 0xE4D7FF, 0xF2F2F2];

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    /// A card of a page of the notebook.
    Page(i64),
    Note(String),
    Picture(Arc<[u8]>),
    /// A titled area that holds what is in it.
    Group(String),
    /// The picture of a map: behind everything, and still (dragging on it
    /// moves the view).
    Map(Arc<[u8]>),
    /// A marker of a page on a map; its point is the bottom middle.
    Pin(i64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: i64,
    pub kind: ItemKind,
    /// Left, top, width, height in canvas units.
    pub rect: [f32; 4],
    /// RGB.
    pub color: u32,
}

impl Item {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let [l, t, w, h] = self.rect;
        x >= l && x <= l + w && y >= t && y <= t + h
    }

    /// Whether `other` lies wholly inside it.
    pub fn holds(&self, other: &Item) -> bool {
        let [l, t, w, h] = self.rect;
        let [ol, ot, ow, oh] = other.rect;
        ol >= l && ot >= t && ol + ow <= l + w && ot + oh <= t + h
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arrow {
    pub id: i64,
    pub from: i64,
    pub to: i64,
    pub label: String,
    pub rel: Rel,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Canvas {
    /// Back to front.
    pub items: Vec<Item>,
    pub arrows: Vec<Arrow>,
}

impl Canvas {
    pub fn new_id(&self) -> i64 {
        self.items.iter().map(|i| i.id).chain(self.arrows.iter().map(|a| a.id)).max().unwrap_or(0) + 1
    }

    pub fn item(&self, id: i64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: i64) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// Adds `kind` at `rect`; groups go behind everything else, and the
    /// map behind them.
    pub fn add(&mut self, kind: ItemKind, rect: [f32; 4], color: u32) -> i64 {
        let id = self.new_id();
        let item = Item { id, kind, rect, color };
        if matches!(item.kind, ItemKind::Map(_)) {
            self.items.insert(0, item);
        } else if matches!(item.kind, ItemKind::Group(_)) {
            let at = self.items.iter().position(|i| !matches!(i.kind, ItemKind::Map(_))).unwrap_or(self.items.len());
            self.items.insert(at, item);
        } else {
            self.items.push(item);
        }
        id
    }

    /// An arrow from item `from` to item `to` (none to itself, nor twice).
    pub fn connect(&mut self, from: i64, to: i64, rel: Rel) -> Option<i64> {
        if from == to || self.item(from).is_none() || self.item(to).is_none() || self.arrows.iter().any(|a| a.from == from && a.to == to) {
            return None;
        }
        let id = self.new_id();
        self.arrows.push(Arrow { id, from, to, label: String::new(), rel });
        Some(id)
    }

    /// Removes items and arrows (and the arrows of removed items).
    pub fn remove(&mut self, ids: &[i64]) {
        self.items.retain(|i| !ids.contains(&i.id));
        let items = &self.items;
        self.arrows.retain(|a| !ids.contains(&a.id) && items.iter().any(|i| i.id == a.from) && items.iter().any(|i| i.id == a.to));
    }

    /// The front-most item at a point (groups only by their title bar
    /// unless nothing else is there; never the map).
    pub fn item_at(&self, x: f32, y: f32) -> Option<i64> {
        let still = |i: &&Item| !matches!(i.kind, ItemKind::Map(_));
        self.items.iter().rev().filter(still).find(|i| !matches!(i.kind, ItemKind::Group(_)) && i.contains(x, y)).or_else(|| self.items.iter().rev().filter(still).find(|i| i.contains(x, y))).map(|i| i.id)
    }

    /// Its map's picture, if it has one.
    pub fn map(&self) -> Option<&Item> {
        self.items.iter().find(|i| matches!(i.kind, ItemKind::Map(_)))
    }

    /// Puts `bytes` as its map (in place of the one it had) at `rect`.
    pub fn set_map(&mut self, bytes: Arc<[u8]>, rect: [f32; 4]) -> i64 {
        let old: Vec<i64> = self.items.iter().filter(|i| matches!(i.kind, ItemKind::Map(_))).map(|i| i.id).collect();
        self.remove(&old);
        self.add(ItemKind::Map(bytes), rect, 0xFFFFFF)
    }

    /// The pages it has markers of, once each.
    pub fn pinned(&self) -> Vec<i64> {
        let mut v: Vec<i64> = Vec::new();
        for i in &self.items {
            if let ItemKind::Pin(p) = i.kind
                && !v.contains(&p)
            {
                v.push(p);
            }
        }
        v
    }

    /// Items inside group `id` (moved with it).
    pub fn inside(&self, id: i64) -> Vec<i64> {
        let Some(g) = self.item(id).filter(|g| matches!(g.kind, ItemKind::Group(_))) else { return Vec::new() };
        self.items.iter().filter(|i| i.id != id && g.holds(i)).map(|i| i.id).collect()
    }

    /// The links its arrows make between pages: (from page, to page, what
    /// it means); and those of canvas page `me` to the pages it has
    /// markers of.
    pub fn page_links(&self, me: i64) -> Vec<(i64, i64, Rel)> {
        let page = |id: i64| match self.item(id).map(|i| &i.kind) {
            Some(ItemKind::Page(p) | ItemKind::Pin(p)) => Some(*p),
            _ => None,
        };
        let mut v: Vec<(i64, i64, Rel)> = self.arrows.iter().filter_map(|a| Some((page(a.from)?, page(a.to)?, a.rel))).filter(|(a, b, _)| a != b).collect();
        for p in self.pinned() {
            if p != me && !v.iter().any(|l| l.0 == me && l.1 == p) {
                v.push((me, p, Rel::Link));
            }
        }
        v
    }

    /// Its words, to search it: notes, group titles and arrow labels.
    pub fn text(&self) -> String {
        let mut out: Vec<&str> = Vec::new();
        for i in &self.items {
            match &i.kind {
                ItemKind::Note(t) | ItemKind::Group(t) if !t.trim().is_empty() => out.push(t),
                _ => {}
            }
        }
        out.extend(self.arrows.iter().map(|a| a.label.as_str()).filter(|l| !l.is_empty()));
        out.join("\n")
    }
}
