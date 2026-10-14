//! Application.GetOpenFilename, GetSaveAsFilename and FileDialog: the
//! application's own file dialog; what the user picks the macro may then
//! open (or save in, from a Save As dialog).

use std::rc::Rc;

use super::{Args, Interp, obj};
use crate::error::{Error, R};
use crate::files::{FilePick, PickKind};
use crate::object::{Obj, Part};
use crate::value::{Array, Value};

/// Dialogs a run may show (a macro looping on one stops).
const MAX_DIALOGS: u32 = 1_000;

/// `"Text Files (*.txt),*.txt,All Files (*.*),*.*"` as (description, patterns).
fn parse_filter(text: &str) -> Vec<(String, Vec<String>)> {
    let parts: Vec<&str> = text.split(',').collect();
    parts.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0].trim().to_string(), patterns(c[1]))).collect()
}

fn patterns(text: &str) -> Vec<String> {
    text.split(';').map(str::trim).filter(|p| !p.is_empty()).map(String::from).collect()
}

/// The filter numbered `index` (from 1) first.
fn chosen_first(mut filters: Vec<(String, Vec<String>)>, index: i64) -> Vec<(String, Vec<String>)> {
    if index > 1 && (index as usize) <= filters.len() {
        let f = filters.remove(index as usize - 1);
        filters.insert(0, f);
    }
    filters
}

impl Interp<'_> {
    /// Shows the dialog: the paths picked, now reachable by the macro.
    fn show_pick(&mut self, pick: FilePick) -> R<Option<Vec<String>>> {
        self.dialogs_shown += 1;
        if self.dialogs_shown > MAX_DIALOGS {
            return Err(Error::app("Too many file dialogs"));
        }
        if matches!(self.wb, crate::interp::Book::Ref(_)) {
            return Err(Error::new(70, "Permission denied: a function called from a cell can't use files"));
        }
        let got = self.paused(|ui| ui.pick_files(&pick)).filter(|p| !p.is_empty());
        let Some(mut paths) = got else { return Ok(None) };
        if !pick.multi {
            paths.truncate(1);
        }
        paths.truncate(10_000);
        for p in &paths {
            self.files.picked(p, pick.kind);
        }
        Ok(Some(paths))
    }

    /// Application.GetOpenFilename(FileFilter, FilterIndex, Title,
    /// ButtonText, MultiSelect): the path (an array of them with
    /// MultiSelect), or False.
    pub(super) fn get_open_filename(&mut self, a: &Args) -> R<Value> {
        let filter = self.arg_str(a, 0, "filefilter")?.unwrap_or_else(|| "All Files (*.*),*.*".into());
        let index = self.arg_i64(a, 1, "filterindex")?.unwrap_or(1);
        let title = self.arg_str(a, 2, "title")?.unwrap_or_default();
        let multi = match a.get(4, "multiselect") {
            Some(v) => self.scalar(v)?.to_bool()?,
            None => false,
        };
        let pick = FilePick { kind: PickKind::Open, multi, title, filters: chosen_first(parse_filter(&filter), index), initial: String::new() };
        Ok(match self.show_pick(pick)? {
            None => Value::Bool(false),
            Some(paths) if multi => Value::Array(Rc::new(Array::from_vec(paths.into_iter().map(Value::str).collect(), 1))),
            Some(mut paths) => Value::str(paths.swap_remove(0)),
        })
    }

    /// Application.GetSaveAsFilename(InitialFilename, FileFilter,
    /// FilterIndex, Title, ButtonText): the path, or False.
    pub(super) fn get_save_as_filename(&mut self, a: &Args) -> R<Value> {
        let initial = self.arg_str(a, 0, "initialfilename")?.unwrap_or_default();
        let filter = self.arg_str(a, 1, "filefilter")?.unwrap_or_else(|| "All Files (*.*),*.*".into());
        let index = self.arg_i64(a, 2, "filterindex")?.unwrap_or(1);
        let title = self.arg_str(a, 3, "title")?.unwrap_or_default();
        let pick = FilePick { kind: PickKind::Save, multi: false, title, filters: chosen_first(parse_filter(&filter), index), initial };
        Ok(match self.show_pick(pick)? {
            None => Value::Bool(false),
            Some(mut paths) => Value::str(paths.swap_remove(0)),
        })
    }

    /// Application.FileDialog(msoFileDialogType).
    pub(super) fn file_dialog(&mut self, a: &Args) -> R<Value> {
        let kind = self.arg_i64(a, 0, "filedialogtype")?.ok_or_else(|| Error::new(449, "Argument not optional"))?;
        if !(1..=4).contains(&kind) {
            return Err(Error::invalid_call());
        }
        Ok(obj(Obj::Part(Box::new(Part::FileDialog(kind as u8)))))
    }

    pub(super) fn dialog_member(&mut self, k: u8, name: &str, a: &Args) -> R<Value> {
        let d = &self.files.dialogs[usize::from(k - 1)];
        Ok(match name {
            "title" => Value::str(d.title.as_str()),
            "allowmultiselect" => Value::Bool(d.multi),
            "initialfilename" => Value::str(d.initial.as_str()),
            "buttonname" => Value::str(d.button.as_str()),
            "filterindex" => Value::Long(d.filter_index as i32),
            "dialogtype" => Value::Long(i32::from(k)),
            "initialview" => Value::Long(1),
            // `.SelectedItems(1)`: an item.
            "filters" => return self.part_get(Part::DialogFilters(k), a),
            "selecteditems" => return self.part_get(Part::DialogItems(k), a),
            "show" => {
                let d = d.clone();
                let filters = if d.kind == 4 { Vec::new() } else { d.filters.iter().map(|(t, e)| (t.clone(), patterns(e))).collect() };
                let pick = FilePick { kind: d.pick_kind(), multi: d.multi && d.kind != 2, title: d.title, filters: chosen_first(filters, d.filter_index), initial: d.initial };
                let got = self.show_pick(pick)?;
                let shown = got.is_some();
                self.files.dialogs[usize::from(k - 1)].selected = got.unwrap_or_default();
                Value::Int(if shown { -1 } else { 0 })
            }
            "execute" => return Err(Error::unsupported("FileDialog.Execute (open or save the file with Workbooks.Open or SaveAs)")),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn dialog_set(&mut self, k: u8, name: &str, v: Value) -> R<()> {
        let d = &mut self.files.dialogs[usize::from(k - 1)];
        match name {
            "title" => d.title = v.to_str()?,
            "allowmultiselect" => d.multi = v.to_bool()?,
            "initialfilename" => d.initial = v.to_str()?,
            "buttonname" => d.button = v.to_str()?,
            "filterindex" => d.filter_index = v.to_i64()?,
            "initialview" => {}
            _ => return Err(Error::no_member(name)),
        }
        Ok(())
    }

    pub(super) fn dialog_filters(&mut self, k: u8, name: &str, a: &Args) -> R<Value> {
        let i = usize::from(k - 1);
        Ok(match name {
            "count" => Value::Long(self.files.dialogs[i].filters.len() as i32),
            "item" => {
                let n = self.arg_i64(a, 0, "index")?.ok_or_else(|| Error::new(449, "Argument not optional"))?;
                if n < 1 || n as usize > self.files.dialogs[i].filters.len() {
                    return Err(Error::subscript());
                }
                obj(Obj::Part(Box::new(Part::DialogFilter(k, n as usize - 1))))
            }
            "add" => {
                if self.files.dialogs[i].kind == 2 || self.files.dialogs[i].kind == 4 {
                    return Err(Error::app("This dialog has no filters to change"));
                }
                let desc = self.arg_str(a, 0, "description")?.ok_or_else(|| Error::new(449, "Argument not optional"))?;
                let ext = self.arg_str(a, 1, "extensions")?.ok_or_else(|| Error::new(449, "Argument not optional"))?;
                let filters = &mut self.files.dialogs[i].filters;
                if filters.len() >= 1_000 {
                    return Err(Error::app("Too many filters"));
                }
                let at = match self.arg_i64(a, 2, "position")? {
                    Some(p) if p >= 1 => (p as usize - 1).min(self.files.dialogs[i].filters.len()),
                    Some(_) => return Err(Error::invalid_call()),
                    None => self.files.dialogs[i].filters.len(),
                };
                self.files.dialogs[i].filters.insert(at, (desc, ext));
                obj(Obj::Part(Box::new(Part::DialogFilter(k, at))))
            }
            "clear" => {
                self.files.dialogs[i].filters.clear();
                Value::Empty
            }
            "delete" => {
                match self.arg_i64(a, 0, "filter")? {
                    Some(n) if n >= 1 && (n as usize) <= self.files.dialogs[i].filters.len() => {
                        self.files.dialogs[i].filters.remove(n as usize - 1);
                    }
                    Some(_) => return Err(Error::subscript()),
                    None => self.files.dialogs[i].filters.clear(),
                }
                Value::Empty
            }
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn dialog_filter(&mut self, k: u8, n: usize, name: &str) -> R<Value> {
        let (desc, ext) = self.files.dialogs[usize::from(k - 1)].filters.get(n).cloned().ok_or_else(Error::subscript)?;
        Ok(match name {
            "description" => Value::str(desc),
            "extensions" => Value::str(ext),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn dialog_items(&mut self, k: u8, name: &str, a: &Args) -> R<Value> {
        Ok(match name {
            "count" => Value::Long(self.files.dialogs[usize::from(k - 1)].selected.len() as i32),
            "item" => {
                let n = self.arg_i64(a, 0, "index")?.ok_or_else(|| Error::new(449, "Argument not optional"))?;
                let items = &self.files.dialogs[usize::from(k - 1)].selected;
                if n < 1 || n as usize > items.len() {
                    return Err(Error::subscript());
                }
                Value::str(items[n as usize - 1].as_str())
            }
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn dialog_items_list(&self, k: u8) -> Vec<Value> {
        self.files.dialogs[usize::from(k - 1)].selected.iter().map(|s| Value::str(s.as_str())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        let f = parse_filter("Text Files (*.txt),*.txt,Data (*.csv;*.tsv),*.csv;*.tsv");
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].1, vec!["*.csv", "*.tsv"]);
        assert_eq!(chosen_first(f, 2)[0].0, "Data (*.csv;*.tsv)");
    }
}
