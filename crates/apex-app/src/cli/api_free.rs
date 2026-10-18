//! `apexdesk api` is part of ApexDesk Empresa.

use super::{Res, error};

pub const API_HELP: &str = "Usage: apexdesk api

The automation interface for scripts. Part of ApexDesk Empresa.
";

pub fn run() -> Res<i32> {
    error("api is part of ApexDesk Empresa")
}
