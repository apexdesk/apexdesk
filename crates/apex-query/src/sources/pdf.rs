//! Tables of a PDF (bank statements, invoices, reports), found by the
//! reader that opens PDFs as documents (`apex_io::pdf`).

use crate::{DataTable, Error};

/// Every table of the PDF, in page order, with where each one is.
pub fn tables_with_pages(data: &[u8], header: bool) -> Result<Vec<(usize, DataTable)>, Error> {
    let found = apex_io::pdf::tables(data, None).map_err(|e| Error::Parse(e.to_string()))?;
    Ok(found.into_iter().map(|t| (t.page, DataTable::from_text_rows(t.rows, header))).collect())
}

pub fn tables(data: &[u8], header: bool) -> Result<Vec<DataTable>, Error> {
    Ok(tables_with_pages(data, header)?.into_iter().map(|(_, t)| t).collect())
}
