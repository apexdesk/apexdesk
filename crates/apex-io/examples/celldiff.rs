//! `celldiff a b`: lists filled cells that are missing or differ between
//! two workbooks.
use apex_io::Loaded;
use apex_sheet::Value;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let load = |p: &str| match apex_io::open(std::path::Path::new(p)).unwrap().0 {
        Loaded::Sheet(wb) => wb,
        _ => panic!("not a sheet"),
    };
    let (x, y) = (load(&a[0]), load(&a[1]));
    let filled = |v: &Value| !matches!(v, Value::Empty) && !matches!(v, Value::Text(t) if t.is_empty());
    let mut shown = 0;
    for (si, s) in x.sheets.iter().enumerate() {
        let other = &y.sheets[si];
        for (c, cell) in s.iter().filter(|(_, c)| filled(c.value())) {
            let theirs = other.get(c).map(|o| o.value().clone()).filter(filled);
            if theirs.is_none() && shown < 15 {
                shown += 1;
                println!("{}!{}: {:?} -> {:?}", s.name, c.to_a1(), cell.content, other.get(c).map(|o| &o.content));
            }
        }
        for (c, cell) in other.iter().filter(|(_, c)| filled(c.value())) {
            if s.get(c).map(|o| o.value().clone()).filter(filled).is_none() && shown < 15 {
                shown += 1;
                println!("{}!{}: extra {:?}", s.name, c.to_a1(), cell.content);
            }
        }
    }
}
