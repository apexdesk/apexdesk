//! Automatic QA: deterministic checks (no AI) over the whole window
//! driven without a screen, with the button audit's harness. Each section
//! is an ignored test `qa_<section>` that writes `<section>.md` (one
//! failure a line) and `<section>.sum` (one line of counts) to
//! `$APEXDESK_QA_OUT`; `scripts/qa.sh` runs them all and joins the
//! reports (see `docs/qa.md`). Failures found are the result, not a
//! failed test: only the QA itself breaking fails.

mod common;
mod model;
mod raster;
mod scenes;

mod a11y;
mod align;
mod behave;
mod i18n;
mod invariants;
mod layout;
mod monkey;
mod screens;
mod stories;
