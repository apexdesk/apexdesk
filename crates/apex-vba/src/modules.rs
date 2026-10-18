//! Code reached through an object: `Module1.Proc`, a sheet by its code name
//! (`Sheet1.Range("A1")`), and the public procedures and variables of a
//! sheet's or the workbook's module (`CCSInfo.Password`,
//! `Worksheets("Data").Refresh`, `ThisWorkbook.Helper`).

use crate::ast::{Arg, ModuleKind};
use crate::error::R;
use crate::interp::{Interp, Owner, owner_for};
use crate::object::Obj;
use crate::value::Value;

impl Interp<'_> {
    /// What a module's name stands for in code: its sheet, the workbook,
    /// or the module itself.
    pub fn module_object(&self, mi: usize) -> Obj {
        let m = &self.project.modules[mi];
        match m.kind {
            ModuleKind::Document if self.word.is_some() && owner_for(m, &self.wb) == Owner::Workbook => Obj::Word(Box::new(crate::word::WObj::Doc)),
            ModuleKind::Document => match owner_for(m, &self.wb) {
                Owner::Sheet(s) => Obj::Sheet(s),
                Owner::Workbook => Obj::Workbook,
                Owner::None => Obj::Module(mi),
            },
            _ => Obj::Module(mi),
        }
    }

    /// The module whose code belongs to `o`.
    fn code_module(&self, o: &Obj) -> Option<usize> {
        let owner = match o {
            Obj::Module(mi) => return Some(*mi),
            Obj::Sheet(s) => Owner::Sheet(*s),
            Obj::Workbook => Owner::Workbook,
            Obj::Word(w) if matches!(**w, crate::word::WObj::Doc) => Owner::Workbook,
            _ => return None,
        };
        self.project.modules.iter().position(|m| m.kind == ModuleKind::Document && owner_for(m, &self.wb) == owner)
    }

    /// `o.name(args)` when `name` is a procedure or a variable of the
    /// module of `o`.
    pub fn code_member(&mut self, o: &Obj, name: &str, args: &[Arg]) -> Option<R<Value>> {
        let mi = self.code_module(o)?;
        if let Some(p) = self.project.modules[mi].proc(name) {
            let p = p.clone();
            return Some(self.call_proc(mi, p, args));
        }
        let s = self.module_var(mi, name)?;
        let v = s.borrow().val.clone();
        Some(if args.is_empty() { Ok(v) } else { self.index_value(v, args) })
    }

    /// `o.name = v` for a variable of the module of `o`.
    pub fn set_code_member(&mut self, o: &Obj, name: &str, args: &[Arg], v: Value) -> Option<R<()>> {
        if !args.is_empty() {
            return None;
        }
        let mi = self.code_module(o)?;
        let s = self.module_var(mi, name)?;
        let set = v.is_object();
        Some(self.store(&s, v, set))
    }
}
