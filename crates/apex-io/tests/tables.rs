use apex_doc::{Align, Border, Borders, Document, Paragraph, RowRule, Table, TableLook, TableMap, VAlign};

fn sample() -> Document {
    let mut t = Table::new(3, 3, 450.0);
    for r in 0..3 {
        for c in 0..3 {
            t.rows[r][c].paras = vec![Paragraph::new(format!("r{r}c{c}"))];
        }
    }
    t.rows[2][1].paras.push(Paragraph::new("segundo"));
    t.props.widths = vec![100.0, 150.0, 200.0];
    t.props.style = Some("GridTable4-Accent1".into());
    t.props.look = TableLook { banded_rows: false, first_col: false, ..TableLook::default() };
    t.props.header_rows = 1;
    t.props.align = Align::Center;
    t.props.row_heights = vec![0.0, 30.0];
    // An exact row and one kept on one page.
    t.props.row_rules = vec![RowRule::default(), RowRule { exact: true, cant_split: false }, RowRule { cant_split: true, exact: false }];
    t.rows[1][1].props.shading = Some(0xFFEE00);
    t.rows[1][1].props.valign = VAlign::Center;
    t.rows[2][2].props.borders = Borders { top: Some(Border::new(2.0, 0xFF0000)), right: Some(Border::NONE), ..Default::default() };
    t.merge(0, 1, 0, 2);
    t.merge(1, 0, 2, 0);
    let mut ps = vec![Paragraph::new("before")];
    ps.extend(t.into_paras(1));
    ps.push(Paragraph::new("after"));
    Document::from_paragraphs(ps)
}

/// Rows of cell texts (paragraphs joined by '/', covered cells as "#").
fn grid(d: &Document, m: &TableMap) -> Vec<Vec<String>> {
    (0..m.rows)
        .map(|r| {
            (0..m.cols)
                .map(|c| {
                    if !m.is_anchor(r, c) {
                        return "#".to_string();
                    }
                    let (a, b) = m.cell(r, c);
                    (a..b).map(|i| d.paragraphs[i].text.clone()).collect::<Vec<_>>().join("/")
                })
                .collect()
        })
        .collect()
}

fn same_tables(back: &Document, orig: &Document, props: bool) {
    let (ta, tb) = (back.tables(), orig.tables());
    assert_eq!(ta.len(), tb.len());
    assert_eq!(back.paragraphs.first().map(|p| p.text.as_str()), Some("before"));
    assert_eq!(back.paragraphs.last().map(|p| p.text.as_str()), Some("after"));
    for (a, b) in ta.iter().zip(&tb) {
        assert_eq!(grid(back, a), grid(orig, b));
        if !props {
            continue;
        }
        assert_eq!(a.props(&back.paragraphs), b.props(&orig.paragraphs));
        for r in 0..b.rows {
            for c in 0..b.cols {
                if b.is_anchor(r, c) {
                    assert_eq!(a.span_of(r, c), b.span_of(r, c), "{r},{c}");
                    let pa = a.cell_props(&back.paragraphs, r, c).cloned().unwrap_or_default();
                    let pb = b.cell_props(&orig.paragraphs, r, c).cloned().unwrap_or_default();
                    assert_eq!(pa, pb, "{r},{c}");
                }
            }
        }
    }
}

#[test]
fn docx_roundtrip() {
    let d = sample();
    let back = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    same_tables(&back, &d, true);
    // Twice gives the same file.
    let again = apex_io::docx::read(&apex_io::docx::write(&back)).unwrap();
    same_tables(&again, &d, true);
}

#[test]
fn odt_roundtrip() {
    let d = sample();
    let back = apex_io::odt::read(&apex_io::odt::write(&d)).unwrap();
    same_tables(&back, &d, true);
}

#[test]
fn markdown_tables() {
    let md = "Intro\n\n| A | B \\| b | C |\n|---|:-:|--:|\n| 1 | **dos** | `x|y` |\n| 4 |\n\nFin\n";
    let d = apex_io::markdown::read(md);
    let t = d.tables();
    assert_eq!(t.len(), 1);
    let m = &t[0];
    assert_eq!(grid(&d, m), vec![vec!["A", "B | b", "C"], vec!["1", "dos", "x|y"], vec!["4", "", ""]]);
    assert_eq!(d.paragraphs[m.cell(0, 1).0].style.align, Align::Center);
    assert_eq!(m.props(&d.paragraphs).map(|p| p.header_rows), Some(1));
    assert_eq!(d.paragraphs.last().map(|p| p.text.as_str()), Some("Fin"));
    let out = apex_io::markdown::write(&d);
    assert_eq!(out, "Intro\n\n| A | B \\| b | C |\n|---|:-:|--:|\n| 1 | **dos** | `x\\|y` |\n| 4 |  |  |\n\nFin\n");
    let back = apex_io::markdown::read(&out);
    assert_eq!(grid(&back, &back.tables()[0]), grid(&d, m));
}

#[test]
fn docx_word_tables() {
    // Merges written the way Word does, a nested table and a style that
    // isn't built in.
    let xml = r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:tbl><w:tblPr><w:tblStyle w:val="Mine"/><w:tblLook w:val="04A0"/></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid>
<w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>ab</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:vMerge w:val="restart"/><w:shd w:val="clear" w:fill="00FF00"/></w:tcPr><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>d</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>in1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>in2</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/></w:tc><w:tc><w:p><w:r><w:t>e</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc></w:tr>
</w:tbl><w:sectPr/></w:body></w:document>"#;
    let styles = r#"<?xml version="1.0"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="table" w:styleId="Mine"><w:name w:val="Mine"/><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="8" w:color="0000FF"/><w:insideH w:val="single" w:sz="4"/></w:tblBorders></w:tblPr></w:style></w:styles>"#;
    let mut z = apex_io::zip::ZipWriter::new();
    z.add("word/document.xml", xml.as_bytes(), true);
    z.add("word/styles.xml", styles.as_bytes(), true);
    let d = apex_io::docx::read(&z.finish()).unwrap();
    let t = d.tables();
    assert_eq!(t.len(), 1);
    let m = &t[0];
    assert_eq!(grid(&d, m), vec![vec!["ab", "#", "c"], vec!["d/in1/in2/", "e", "#"]]);
    // The nested table stays a table in the cell.
    let all = d.all_tables();
    assert_eq!(all.len(), 2);
    assert_eq!((all[1].depth, all[1].rows, all[1].cols), (1, 1, 2));
    assert_eq!(m.span_of(0, 0), (1, 2));
    assert_eq!(m.span_of(0, 2), (2, 1));
    let p = m.props(&d.paragraphs).unwrap();
    assert_eq!(p.header_rows, 1);
    assert_eq!(p.widths, vec![100.0; 3]);
    assert_eq!(p.style, None);
    assert_eq!(p.borders.top, Some(Border::new(1.0, 0x0000FF)));
    assert_eq!(p.borders.inside_h, Some(Border::new(0.5, 0)));
    assert!(p.look.first_row && p.look.first_col && !p.look.banded_cols);
    assert_eq!(m.cell_props(&d.paragraphs, 0, 2).and_then(|c| c.shading), Some(0x00FF00));
    // The document ends with a paragraph after the table.
    assert!(d.paragraphs.last().is_some_and(|p| p.cell.is_none()));
}
