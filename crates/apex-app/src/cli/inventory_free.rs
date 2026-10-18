//! `apexdesk inventory` is part of ApexDesk Empresa.

use super::args::Parsed;
use super::{Res, error};

pub const INVENTORY_HELP: &str = "Usage: apexdesk inventory FOLDER...

Lists the office files under folders. Part of ApexDesk Empresa.
";

pub fn inventory(_a: &Parsed) -> Res<i32> {
    error("inventory is part of ApexDesk Empresa")
}
