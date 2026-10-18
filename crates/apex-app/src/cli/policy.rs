//! `apexdesk policy`: the policies in force and where they come from, and
//! the templates administrators set them with.

use std::path::Path;

use super::args::Parsed;
use super::{Fail, Res, emit, usage};
use crate::policy::{self, ITEMS, Kind};

pub const POLICY_HELP: &str = "Usage: apexdesk policy [--templates DIR]

Prints the policies in force (settings fixed by the organization), with
the registry key, profile or file each comes from. They are read from:
  Windows  HKLM, then HKCU\\Software\\Policies\\ApexDesk
  macOS    managed preferences of com.github.apexdesk.apexdesk
  Linux    /etc/apexdesk/policy and /etc/apexdesk/policy.d/*.conf
  and last the file named by the APEXDESK_POLICY variable.

Options:
      --templates DIR   write the Windows ADMX/ADML templates, a sample
                        macOS profile and a sample Linux policy file
      --list            list every policy and its values
";

pub fn policy(a: &Parsed) -> Res<i32> {
    if !a.pos.is_empty() {
        return usage(format!("unexpected argument '{}'", a.pos[0]));
    }
    if let Some(dir) = a.value("templates") {
        for (path, text) in policy::admx::files() {
            let p = Path::new(dir).join(&path);
            if let Some(parent) = p.parent() {
                apex_fs::create_dir_all(parent).map_err(|e| Fail::Error(format!("{}: {e}", parent.display())))?;
            }
            crate::common::write_out(&p, text).map_err(|e| Fail::Error(format!("{}: {e}", p.display())))?;
            emit(&format!("{}\n", p.display()));
        }
        return Ok(0);
    }
    if a.flag("list") {
        for i in ITEMS {
            let values = match i.kind {
                Kind::Bool => "0 or 1".to_string(),
                Kind::Number(lo, hi) => format!("{lo} to {hi}"),
                Kind::Choice(c) => c.join(", "),
                Kind::List => "texts (repeat the name)".into(),
                Kind::Folder => "a folder".into(),
                Kind::Text => "text".into(),
            };
            emit(&format!("{:<20} {values}\n", i.name));
        }
        return Ok(0);
    }
    let p = policy::current();
    if p.set.is_empty() {
        emit("No policies are set.\n");
    }
    for s in &p.set {
        emit(&format!("{} = {}    [{}]\n", s.item.name, s.values.join("; "), s.source));
    }
    for i in &p.ignored {
        emit(&format!("ignored: {i}\n"));
    }
    for n in &p.unlicensed {
        emit(&format!("not applied: {n} (needs a business license)\n"));
    }
    Ok(0)
}
