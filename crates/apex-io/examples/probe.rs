//! Loads a file, prints a summary and re-saves it in every compatible format.
//! Usage: probe <input> <out-dir>

use std::path::Path;
use std::time::Instant;

use apex_io::{Format, Loaded};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let input = Path::new(&args[1]);
    let out = Path::new(&args[2]);
    let t = Instant::now();
    let (loaded, fmt) = apex_io::open(input).expect("open");
    println!("{:?} loaded in {:?}", fmt, t.elapsed());
    let stem = input.file_stem().unwrap().to_string_lossy();
    match loaded {
        Loaded::Sheet(wb) => {
            for s in &wb.sheets {
                println!("sheet {:?}: {} cells, extent {:?}", s.name, s.cell_count(), s.extent());
                for (at, cell) in s.iter().take(8) {
                    println!("  {} = {:?}", at.to_a1(), cell.content);
                }
            }
            for f in [Format::Xlsx, Format::Ods, Format::Csv] {
                let p = out.join(format!("{}.{}", stem, f.extension()));
                let t = Instant::now();
                apex_io::save_sheet(&wb, &p, f).expect("save");
                println!("saved {} in {:?}", p.display(), t.elapsed());
            }
        }
        Loaded::Doc(doc) => {
            println!("{} paragraphs, page {:?}, meta {:?}", doc.paragraphs.len(), doc.page, doc.meta);
            for p in doc.paragraphs.iter().take(12) {
                println!("  {:?} {:?} runs={}", p.style.kind, p.text.chars().take(60).collect::<String>(), p.runs.len());
            }
            for (pos, img) in doc.images() {
                println!("  image at {:?}: {} bytes, {:.1}x{:.1}pt, {:?}, crop {:?}, rot {}, {:?}", pos, img.data.len(), img.width, img.height, img.wrap, img.crop, img.rotation, img.alt);
            }
            for f in [Format::Docx, Format::Odt, Format::Markdown] {
                let p = out.join(format!("{}.{}", stem, f.extension()));
                apex_io::save_doc(&doc, &p, f).expect("save");
                println!("saved {}", p.display());
            }
        }
        Loaded::Slides(deck) => {
            println!("{} slides, {} masters, {:.0}x{:.0}pt, meta {:?}", deck.slides.len(), deck.masters.len(), deck.width, deck.height, deck.meta);
            for (i, s) in deck.slides.iter().enumerate().take(12) {
                println!("  slide {}: {:?}, {} objects", i + 1, s.title(), s.objects.len());
            }
            let p = out.join(format!("{stem}.pptx"));
            apex_io::save_slides(&deck, &p, Format::Pptx).expect("save");
            println!("saved {}", p.display());
        }
    }
}
