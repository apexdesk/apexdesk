//! Prints what a presentation's slides resolve to: each object's frame,
//! fill, line and first run, and the background (a debugging aid).
//!
//! `cargo run -p apex-io --example slides_dump -- FILE.pptx [SLIDE]`

use apex_slides::resolve::Chain;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&args[1]).expect("read");
    let deck = apex_io::pptx::read(&data).expect("pptx");
    let only: Option<usize> = args.get(2).and_then(|s| s.parse().ok());
    for (i, s) in deck.slides.iter().enumerate() {
        if only.is_some_and(|n| n != i + 1) {
            continue;
        }
        let ch = Chain::for_slide(&deck, s);
        println!("slide {} layout {}/{} bg {:?}", i + 1, s.master, s.layout, ch.background());
        for o in &s.objects {
            println!("  {} {:?} frame {:?}", o.name, o.ph.map(|p| p.kind), ch.frame(o));
            println!("    fill {:?}", ch.fill(o));
            println!("    line {:?}", ch.line(o));
        }
    }
}
