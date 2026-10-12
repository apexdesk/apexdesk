//! apex-bench: runs ApexDesk over a corpus of real-world files (see
//! docs/plan-bloque5.md) and reports what fails: files that don't open,
//! hang or crash, lose content on save, or disagree with what Office
//! computed and stored in them. A development tool, not shipped.

mod big;
mod fuzz;
mod oracle;
mod oracle_formulas;
mod oracle_pages;
mod oracle_proc;
mod oracle_zip;
mod run;
mod validate;

use std::process::ExitCode;

const USAGE: &str = "apex-bench: test battery over real-world files.

Usage:
  apex-bench run DIR [options]               open, save and compare every file
                                             (apex-bench run --help)
  apex-bench oracle DIR [--out REPORT_DIR]   compare with what Office stored in the files
             [--jobs N] [--limit N] [--only formulas|pages] [--apexdesk PATH] [--reuse]
  apex-bench oracle-cells FILE SHEET!A1...   formula, saved and recalculated value of cells
  apex-bench validate FILES|DIR [--out REPORT_DIR] [--saved] [--limit N] [--tools DIR]
                                             save in docx/xlsx/odt/ods and validate with
                                             the Open XML SDK and the ODF validator
  apex-bench fuzz [SEEDS...] [--time T]      mutation fuzzing of the readers (--help for more)
  apex-bench big [--out DIR]                 open, recalculate, paginate and save big files
  apex-bench judge DIR [--soffice PATH]      presentations against LibreOffice Impress:
                                             pages, words and round trips (--help for more)

Corpora are downloaded with tools/corpus.sh into target/corpus/.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => run::main(&args[1..]),
        Some("oracle") => oracle::main(&args[1..]),
        Some("oracle-file") => oracle::file_main(&args[1..]),
        Some("oracle-cells") => oracle_formulas::cells_main(&args[1..]),
        Some("validate") => validate::main(&args[1..]),
        Some("save-as") => validate::save_main(&args[1..]),
        // The child process of `run` (one file).
        Some("__run-worker") => run::worker_main(&args[1..]),
        Some("fuzz") => fuzz::main(&args[1..]),
        Some("big") => big::main(&args[1..]),
        Some("judge") => run::judge_main(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("apex-bench: {e}");
            ExitCode::FAILURE
        }
    }
}
