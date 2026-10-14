//! Functions of a workbook's macros in its cells (`=IVA(A1)`): given to
//! the workbook while its macros may run (on, enabled for the file, not
//! in Protected View), taken away when not; then the cells keep their
//! last values, as Excel does. The functions of the personal macro
//! workbook and the add-ins come after the workbook's own, in every
//! workbook but those in Protected View.

use std::sync::Arc;

use super::super::{App, View};
use super::{allowed, sources};

impl App {
    pub(super) fn sync_udfs(&mut self) {
        let libs = self.libraries();
        for i in 0..self.tabs.len() {
            let id = self.tabs[i].id;
            let protected = self.is_protected(id);
            let on = allowed(&self.settings, &self.tabs, id) && self.macro_run.is_enabled(id) && !protected;
            let file_name = self.tabs[i].name();
            let file_path = self.tabs[i].path.as_ref().and_then(|p| p.parent()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let View::Sheet(v) = &mut self.tabs[i].view else { continue };
            let bin = v.wb.macros.project.clone().filter(|_| on);
            // Its own file isn't a library of its own.
            let libs: Vec<apex_vba::Library> = libs.iter().filter(|l| !protected && !l.is(&file_name)).cloned().collect();
            let want: Vec<usize> = bin.iter().map(|b| Arc::as_ptr(b) as usize).chain(libs.iter().map(|l| Arc::as_ptr(&l.sources) as usize)).collect();
            let had = self.macro_run.udf.insert(id, want.clone());
            if had.as_ref() == Some(&want) || (had.is_none() && want.is_empty() && v.wb.udf.is_none()) {
                continue;
            }
            let opts = apex_vba::Options { file_name, file_path, user_name: crate::doc_view::user_name(), utc_offset: crate::common::local_utc_offset(), ..apex_vba::Options::default() };
            let own = bin.and_then(|b| sources(&b).ok()).map(Arc::new);
            let functions = apex_vba::udf::Functions::with_libraries(own, &libs, &opts);
            if functions.is_none() && v.wb.udf.is_none() {
                continue;
            }
            v.wb.set_udf(functions.map(|f| Arc::new(f) as Arc<dyn apex_sheet::udf::Udf>));
        }
        let tabs: Vec<u64> = self.tabs.iter().map(|t| t.id).collect();
        self.macro_run.udf.retain(|id, _| tabs.contains(id));
    }
}
