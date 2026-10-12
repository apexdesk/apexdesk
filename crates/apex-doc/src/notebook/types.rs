//! Page types: a kind of page (a character, a place…) with the fields
//! each page of it fills in, and a template page its new pages start
//! from.

use super::{Notebook, SECTION_COLORS};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FieldKind {
    #[default]
    Text,
    Number,
    /// yyyy-mm-dd.
    Date,
    /// One of the field's options.
    Choice,
    /// Yes or no ("1" or "").
    Check,
}

impl FieldKind {
    pub const ALL: [FieldKind; 5] = [FieldKind::Text, FieldKind::Number, FieldKind::Date, FieldKind::Choice, FieldKind::Check];

    pub fn key(self) -> &'static str {
        match self {
            FieldKind::Text => "text",
            FieldKind::Number => "number",
            FieldKind::Date => "date",
            FieldKind::Choice => "choice",
            FieldKind::Check => "check",
        }
    }

    pub fn from_key(k: &str) -> FieldKind {
        FieldKind::ALL.into_iter().find(|f| f.key() == k).unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Field {
    pub name: String,
    pub kind: FieldKind,
    /// The choices of a `Choice` field.
    pub options: Vec<String>,
}

impl Field {
    pub fn new(name: &str, kind: FieldKind) -> Field {
        Field { name: name.to_string(), kind, options: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageType {
    pub id: i64,
    pub name: String,
    /// RGB.
    pub color: u32,
    pub fields: Vec<Field>,
    /// The page whose text new pages of this type start with.
    pub template: Option<i64>,
}

/// Tabs and line breaks would break the way fields are kept.
fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// A type's fields as kept in the file: one per line,
/// "kind\tname\toption|option".
pub fn fields_key(fields: &[Field]) -> String {
    fields.iter().map(|f| format!("{}\t{}\t{}", f.kind.key(), clean(&f.name), f.options.iter().map(|o| clean(o).replace('|', "/")).collect::<Vec<_>>().join("|"))).collect::<Vec<_>>().join("\n")
}

pub fn fields_from_key(s: &str) -> Vec<Field> {
    s.lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let kind = FieldKind::from_key(it.next()?);
            let name = it.next()?.to_string();
            let options = it.next().unwrap_or("").split('|').filter(|o| !o.is_empty()).map(String::from).collect();
            Some(Field { name, kind, options })
        })
        .collect()
}

/// A page's values as kept in the file: "name\tvalue" per line.
pub fn values_key(values: &[(String, String)]) -> String {
    values.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k}\t{v}")).collect::<Vec<_>>().join("\n")
}

pub fn values_from_key(s: &str) -> Vec<(String, String)> {
    s.lines().filter_map(|l| l.split_once('\t')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// A number as typed ("1.5", "1,5"), to sort and compare.
pub fn number(v: &str) -> Option<f64> {
    v.trim().replace(',', ".").parse().ok()
}

impl Notebook {
    pub fn page_type(&self, id: i64) -> Option<&PageType> {
        self.types.iter().find(|t| t.id == id)
    }

    pub fn page_type_mut(&mut self, id: i64) -> Option<&mut PageType> {
        self.types.iter_mut().find(|t| t.id == id)
    }

    /// A new type without fields. Returns its id.
    pub fn add_type(&mut self, name: &str) -> i64 {
        let id = self.types.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        let color = SECTION_COLORS[self.types.len() % SECTION_COLORS.len()];
        self.types.push(PageType { id, name: clean(name), color, fields: Vec::new(), template: None });
        id
    }

    /// Removes type `id`; its pages become plain pages (keeping their
    /// values, should it come back with undo).
    pub fn delete_type(&mut self, id: i64) {
        self.types.retain(|t| t.id != id);
        for p in &mut self.pages {
            if p.ptype == Some(id) {
                p.ptype = None;
            }
        }
    }

    /// Renames field `old` of type `ty`, and the values its pages have.
    pub fn rename_field(&mut self, ty: i64, old: &str, new: &str) {
        let new = clean(new);
        if new.is_empty() || old == new {
            return;
        }
        let Some(t) = self.page_type_mut(ty) else { return };
        if t.fields.iter().any(|f| f.name == new) {
            return;
        }
        if let Some(f) = t.fields.iter_mut().find(|f| f.name == old) {
            f.name.clone_from(&new);
        }
        for p in &mut self.pages {
            if p.ptype == Some(ty) {
                for (k, _) in &mut p.fields {
                    if k == old {
                        k.clone_from(&new);
                    }
                }
            }
        }
    }

    /// Page `page`'s value of field `name` ("" when it has none).
    pub fn field_value(&self, page: i64, name: &str) -> &str {
        self.page(page).and_then(|p| p.fields.iter().find(|(k, _)| k == name)).map_or("", |(_, v)| v.as_str())
    }

    pub fn set_field(&mut self, page: i64, name: &str, value: &str) {
        let Some(p) = self.page_mut(page) else { return };
        let value = clean(value.trim());
        match p.fields.iter_mut().find(|(k, _)| k == name) {
            Some((_, v)) => *v = value,
            None if !value.is_empty() => p.fields.push((name.to_string(), value)),
            None => {}
        }
        p.fields.retain(|(_, v)| !v.is_empty());
    }

    /// The live pages of type `ty`, by title.
    pub fn pages_of_type(&self, ty: i64) -> Vec<i64> {
        let mut v: Vec<&super::Page> = self.pages.iter().filter(|p| p.ptype == Some(ty) && self.is_live(p.id)).collect();
        v.sort_by_key(|p| super::fold(&p.title));
        v.into_iter().map(|p| p.id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_keep_fields_and_rename_values() {
        let mut nb = Notebook::default();
        let s = nb.add_section("Notes", 0);
        let ana = nb.add_page(s, None, None, "Ana", 0);
        let t = nb.add_type("Personaje");
        let fields = vec![Field::new("Edad", FieldKind::Number), Field { name: "Rol".into(), kind: FieldKind::Choice, options: vec!["Protagonista".into(), "Secundario".into()] }];
        assert_eq!(fields_from_key(&fields_key(&fields)), fields);
        nb.page_type_mut(t).unwrap().fields = fields;
        nb.page_mut(ana).unwrap().ptype = Some(t);
        nb.set_field(ana, "Edad", "34\t");
        assert_eq!(nb.field_value(ana, "Edad"), "34");
        nb.rename_field(t, "Edad", "Años");
        assert_eq!(nb.field_value(ana, "Años"), "34");
        assert_eq!(values_from_key(&values_key(&nb.page(ana).unwrap().fields)), vec![("Años".to_string(), "34".to_string())]);
        assert_eq!(nb.pages_of_type(t), vec![ana]);
        nb.set_field(ana, "Años", "");
        assert!(nb.page(ana).unwrap().fields.is_empty());
        nb.delete_type(t);
        assert_eq!(nb.page(ana).unwrap().ptype, None);
        assert_eq!(number("1,5"), Some(1.5));
    }
}
