//! Generates large test files: `gen_big <out-dir>` writes big.xlsx
//! (100k rows, formulas, custom row heights) and big.docx (~300 pages).

use apex_doc::{Document, ParaKind, ParaStyle, Paragraph};
use apex_io::Format;
use apex_sheet::{CellRef, Workbook};

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: gen_big <out-dir>"));
    let mut wb = Workbook::new();
    let rows = 100_000u32;
    let mut inputs = Vec::new();
    for r in 0..rows {
        inputs.push((CellRef::new(r, 0), format!("Item {r}")));
        for c in 1..6 {
            inputs.push((CellRef::new(r, c), format!("{}", (r * 7 + c * 13) % 1000)));
        }
        inputs.push((CellRef::new(r, 6), format!("=SUM(B{0}:F{0})", r + 1)));
    }
    wb.set_inputs(0, inputs);
    for r in 0..rows {
        wb.sheets[0].row_heights.insert(r, 20.0 + (r % 3) as f32);
    }
    apex_io::save_sheet(&wb, &dir.join("big.xlsx"), Format::Xlsx).unwrap();

    let mut paras = Vec::new();
    for i in 0..3000 {
        if i % 20 == 0 {
            paras.push(Paragraph::new(format!("Chapter {}", i / 20 + 1)).with_style(ParaStyle { kind: ParaKind::Heading(1), ..Default::default() }));
        }
        paras.push(Paragraph::new(format!(
            "Paragraph {i}. Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat."
        )));
    }
    apex_io::save_doc(&Document::from_paragraphs(paras), &dir.join("big.docx"), Format::Docx).unwrap();
    println!("written to {}", dir.display());
}
