//! What the binary spreadsheet formats (.xls BIFF8 and .xlsb BIFF12) share:
//! the parsed formulas (`rgce` tokens), decompiled back into the formula
//! text of an .xlsx cell, the built-in function table and the color
//! palette.

use std::sync::LazyLock;

use apex_sheet::{CellRef, ErrorKind};

/// Built-in functions by their index in the file (`Ftab`). `_` marks unused
/// slots; 255 calls an add-in or newer function named by its first argument.
const FTAB: &str = "COUNT IF ISNA ISERROR SUM AVERAGE MIN MAX ROW COLUMN NA NPV STDEV DOLLAR FIXED SIN COS TAN ATAN PI SQRT EXP LN LOG10 ABS INT SIGN ROUND LOOKUP INDEX \
REPT MID LEN VALUE TRUE FALSE AND OR NOT MOD DCOUNT DSUM DAVERAGE DMIN DMAX DSTDEV VAR DVAR TEXT LINEST TREND LOGEST GROWTH GOTO HALT RETURN PV FV NPER PMT \
RATE MIRR IRR RAND MATCH DATE TIME DAY MONTH YEAR WEEKDAY HOUR MINUTE SECOND NOW AREAS ROWS COLUMNS OFFSET ABSREF RELREF ARGUMENT SEARCH TRANSPOSE ERROR \
STEP TYPE ECHO SET.NAME CALLER DEREF WINDOWS SERIES DOCUMENTS ACTIVE.CELL SELECTION RESULT ATAN2 ASIN ACOS CHOOSE HLOOKUP VLOOKUP LINKS INPUT ISREF \
GET.FORMULA GET.NAME SET.VALUE LOG EXEC CHAR LOWER UPPER PROPER LEFT RIGHT EXACT TRIM REPLACE SUBSTITUTE CODE NAMES DIRECTORY FIND CELL ISERR ISTEXT \
ISNUMBER ISBLANK T N FOPEN FCLOSE FSIZE FREADLN FREAD FWRITELN FWRITE FPOS DATEVALUE TIMEVALUE SLN SYD DDB GET.DEF REFTEXT TEXTREF INDIRECT REGISTER CALL \
ADD.BAR ADD.MENU ADD.COMMAND ENABLE.COMMAND CHECK.COMMAND RENAME.COMMAND SHOW.BAR DELETE.MENU DELETE.COMMAND GET.CHART.ITEM DIALOG.BOX CLEAN MDETERM \
MINVERSE MMULT FILES IPMT PPMT COUNTA CANCEL.KEY FOR WHILE BREAK NEXT INITIATE REQUEST POKE EXECUTE TERMINATE RESTART HELP GET.BAR PRODUCT FACT GET.CELL \
GET.WORKSPACE GET.WINDOW GET.DOCUMENT DPRODUCT ISNONTEXT GET.NOTE NOTE STDEVP VARP DSTDEVP DVARP TRUNC ISLOGICAL DCOUNTA DELETE.BAR UNREGISTER _ _ USDOLLAR \
FINDB SEARCHB REPLACEB LEFTB RIGHTB MIDB LENB ROUNDUP ROUNDDOWN ASC DBCS RANK _ _ ADDRESS DAYS360 TODAY VDB ELSE ELSE.IF END.IF FOR.CELL MEDIAN SUMPRODUCT \
SINH COSH TANH ASINH ACOSH ATANH DGET CREATE.OBJECT VOLATILE LAST.ERROR CUSTOM.UNDO CUSTOM.REPEAT FORMULA.CONVERT GET.LINK.INFO TEXT.BOX INFO GROUP \
GET.OBJECT DB PAUSE _ _ RESUME FREQUENCY ADD.TOOLBAR DELETE.TOOLBAR _ RESET.TOOLBAR EVALUATE GET.TOOLBAR GET.TOOL SPELLING.CHECK ERROR.TYPE APP.TITLE \
WINDOW.TITLE SAVE.TOOLBAR ENABLE.TOOL PRESS.TOOL REGISTER.ID GET.WORKBOOK AVEDEV BETADIST GAMMALN BETAINV BINOMDIST CHIDIST CHIINV COMBIN CONFIDENCE \
CRITBINOM EVEN EXPONDIST FDIST FINV FISHER FISHERINV FLOOR GAMMADIST GAMMAINV CEILING HYPGEOMDIST LOGNORMDIST LOGINV NEGBINOMDIST NORMDIST NORMSDIST NORMINV \
NORMSINV STANDARDIZE ODD PERMUT POISSON TDIST WEIBULL SUMXMY2 SUMX2MY2 SUMX2PY2 CHITEST CORREL COVAR FORECAST FTEST INTERCEPT PEARSON RSQ STEYX SLOPE TTEST \
PROB DEVSQ GEOMEAN HARMEAN SUMSQ KURT SKEW ZTEST LARGE SMALL QUARTILE PERCENTILE PERCENTRANK MODE TRIMMEAN TINV _ MOVIE.COMMAND GET.MOVIE CONCATENATE POWER \
PIVOT.ADD.DATA GET.PIVOT.TABLE GET.PIVOT.FIELD GET.PIVOT.ITEM RADIANS DEGREES SUBTOTAL SUMIF COUNTIF COUNTBLANK SCENARIO.GET OPTIONS.LISTS.GET ISPMT DATEDIF \
DATESTRING NUMBERSTRING ROMAN OPEN.DIALOG SAVE.DIALOG VIEW.GET GETPIVOTDATA HYPERLINK PHONETIC AVERAGEA MAXA MINA STDEVPA VARPA STDEVA VARA BAHTTEXT \
THAIDAYOFWEEK THAIDIGIT THAIMONTHOFYEAR THAINUMSOUND THAINUMSTRING THAISTRINGLENGTH ISTHAIDIGIT ROUNDBAHTDOWN ROUNDBAHTUP THAIYEAR RTD CUBEVALUE CUBEMEMBER \
CUBEMEMBERPROPERTY CUBERANKEDMEMBER HEX2BIN HEX2DEC HEX2OCT DEC2BIN DEC2HEX DEC2OCT OCT2BIN OCT2HEX OCT2DEC BIN2DEC BIN2OCT BIN2HEX IMSUB IMDIV IMPOWER IMABS \
IMSQRT IMLN IMLOG2 IMLOG10 IMSIN IMCOS IMEXP IMARGUMENT IMCONJUGATE IMAGINARY IMREAL COMPLEX IMSUM IMPRODUCT SERIESSUM FACTDOUBLE SQRTPI QUOTIENT DELTA \
GESTEP ISEVEN ISODD MROUND ERF ERFC BESSELJ BESSELK BESSELY BESSELI XIRR XNPV PRICEMAT YIELDMAT INTRATE RECEIVED DISC PRICEDISC YIELDDISC TBILLEQ TBILLPRICE \
TBILLYIELD PRICE YIELD DOLLARDE DOLLARFR NOMINAL EFFECT CUMPRINC CUMIPMT EDATE EOMONTH YEARFRAC COUPDAYBS COUPDAYS COUPDAYSNC COUPNCD COUPNUM COUPPCD DURATION \
MDURATION ODDLPRICE ODDLYIELD ODDFPRICE ODDFYIELD RANDBETWEEN WEEKNUM AMORDEGRC AMORLINC CONVERT ACCRINT ACCRINTM WORKDAY NETWORKDAYS GCD MULTINOMIAL LCM \
FVSCHEDULE CUBEKPIMEMBER CUBESET CUBESETCOUNT IFERROR COUNTIFS SUMIFS AVERAGEIF AVERAGEIFS";

static FUNCS: LazyLock<Vec<&'static str>> = LazyLock::new(|| FTAB.split_whitespace().collect());

fn func_name(i: u16) -> Option<&'static str> {
    FUNCS.get(i as usize).copied().filter(|n| *n != "_")
}

/// The argument count of the functions with a fixed one (`PtgFunc`).
fn fixed_args(i: u16) -> Option<usize> {
    Some(match i {
        10 | 19 | 34 | 35 | 63 | 74 | 221 => 0,
        2 | 3 | 15..=18 | 20..=26 | 32 | 33 | 38 | 67..=69 | 71..=73 | 75..=77 | 83 | 86 | 98 | 99 | 105 | 111..=114 | 118 | 121 | 126..=131 | 140 | 141 | 162..=164
        | 184 | 190 | 198 | 211 | 214 | 215 | 229..=234 | 244 | 261 | 271 | 279 | 283 | 284 | 294 | 296 | 298 | 342 | 343 | 347 | 352 | 360 | 368 => 1,
        27 | 30 | 39 | 48 | 97 | 117 | 165 | 212 | 213 | 252 | 274..=276 | 285 | 288 | 299 | 303..=308 | 310..=315 | 325..=328 | 331 | 332 | 337 | 346 | 353 => 2,
        31 | 40..=45 | 47 | 61 | 65 | 66 | 142 | 189 | 195 | 196 | 199 | 210 | 235 | 277 | 278 | 280..=282 | 287 | 290..=292 | 295 | 297 | 300 | 301 | 309 | 351 => 3,
        119 | 143 | 207 | 273 | 286 | 289 | 293 | 302 | 316 | 350 => 4,
        _ => return None,
    })
}

pub(crate) fn error_kind(code: u8) -> ErrorKind {
    match code {
        0x00 => ErrorKind::Null,
        0x07 => ErrorKind::Div0,
        0x0F => ErrorKind::Value,
        0x17 => ErrorKind::Ref,
        0x1D => ErrorKind::Name,
        0x24 => ErrorKind::Num,
        _ => ErrorKind::Na,
    }
}

fn error_text(code: u8) -> &'static str {
    match code {
        0x00 => "#NULL!",
        0x07 => "#DIV/0!",
        0x0F => "#VALUE!",
        0x17 => "#REF!",
        0x1D => "#NAME?",
        0x24 => "#NUM!",
        _ => "#N/A",
    }
}

/// The 56-color palette of .xls files (indexes 8 to 63), before a
/// `PALETTE` record changes it.
pub(crate) const PALETTE: [u32; 56] = [
    0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF, 0x800000, 0x008000, 0x000080, 0x808000, 0x800080, 0x008080, 0xC0C0C0, 0x808080,
    0x9999FF, 0x993366, 0xFFFFCC, 0xCCFFFF, 0x660066, 0xFF8080, 0x0066CC, 0xCCCCFF, 0x000080, 0xFF00FF, 0xFFFF00, 0x00FFFF, 0x800080, 0x800000, 0x008080, 0x0000FF,
    0x00CCFF, 0xCCFFFF, 0xCCFFCC, 0xFFFF99, 0x99CCFF, 0xFF99CC, 0xCC99FF, 0xFFCC99, 0x3366FF, 0x33CCCC, 0x99CC00, 0xFFCC00, 0xFF9900, 0xFF6600, 0x666699, 0x969696,
    0x003366, 0x339966, 0x003300, 0x333300, 0x993300, 0x993366, 0x333399, 0x333333,
];

/// A sheet name as formulas write it: quoted when it isn't a plain word.
pub(crate) fn quote_sheet(name: &str) -> String {
    let plain = !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && CellRef::parse(name).is_none();
    if plain { name.to_string() } else { format!("'{}'", name.replace('\'', "''")) }
}

fn col_letters(mut c: u32) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// Little-endian reading.
pub(crate) struct Cur<'a> {
    pub d: &'a [u8],
    pub p: usize,
}

impl<'a> Cur<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Cur { d, p: 0 }
    }
    pub fn left(&self) -> usize {
        self.d.len().saturating_sub(self.p)
    }
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let b = self.d.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(b)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.bytes(2)?.try_into().ok()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }
    pub fn i32(&mut self) -> Option<i32> {
        Some(self.u32()? as i32)
    }
    pub fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.bytes(8)?.try_into().ok()?))
    }
    pub fn skip(&mut self, n: usize) -> Option<()> {
        self.bytes(n).map(|_| ())
    }
    /// UTF-16 text of `n` characters.
    pub fn utf16(&mut self, n: usize) -> Option<String> {
        let b = self.bytes(n.checked_mul(2)?)?;
        Some(String::from_utf16_lossy(&b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()))
    }
    /// BIFF8 characters after the flags byte: Latin-1 or UTF-16.
    pub fn chars8(&mut self, n: usize, wide: bool) -> Option<String> {
        if wide { self.utf16(n) } else { Some(self.bytes(n)?.iter().map(|&b| b as char).collect()) }
    }
}

/// How a formula refers to things outside its cell.
pub(crate) struct Refs<'a> {
    /// .xlsb (BIFF12) rather than .xls (BIFF8).
    pub biff12: bool,
    /// Excel 5.0/95 (BIFF5): 8-bit strings and smaller references.
    pub biff5: bool,
    /// The cell a shared formula is read for (relative `PtgRefN` tokens).
    pub base: CellRef,
    /// A defined name's formula: relative references count from A1.
    pub in_name: bool,
    /// `ixti` -> the sheet prefix (`Sheet1`, `'A b'`, `Sheet1:Sheet3`); in
    /// BIFF5 the first and last sheet indexes (16 bits each).
    pub sheet: &'a dyn Fn(u32) -> Option<String>,
    /// Defined names, from 1.
    pub name: &'a dyn Fn(u32) -> Option<String>,
    /// Names in other books and add-ins (newer functions): `ixti`, index.
    pub extern_name: &'a dyn Fn(u16, u32) -> Option<String>,
}

impl Refs<'_> {
    fn max_row(&self) -> u32 {
        if self.biff12 {
            1_048_575
        } else if self.biff5 {
            16_383
        } else {
            65_535
        }
    }
    fn max_col(&self) -> u32 {
        if self.biff12 { 16_383 } else { 255 }
    }

    fn row(&self, c: &mut Cur) -> Option<u32> {
        if self.biff12 { c.u32() } else { c.u16().map(u32::from) }
    }

    /// A reference's row and column word (flags in the column's top bits).
    fn loc(&self, c: &mut Cur) -> Option<(u32, u16)> {
        if self.biff5 {
            let row = c.u16()?;
            let col = c.u8()?;
            Some((u32::from(row & 0x3FFF), u16::from(col) | (row & 0xC000)))
        } else {
            let row = self.row(c)?;
            Some((row, c.u16()?))
        }
    }

    /// An area's corners.
    fn area_loc(&self, c: &mut Cur) -> Option<((u32, u16), (u32, u16))> {
        if self.biff5 {
            let (r1, r2) = (c.u16()?, c.u16()?);
            let (c1, c2) = (c.u8()?, c.u8()?);
            Some(((u32::from(r1 & 0x3FFF), u16::from(c1) | (r1 & 0xC000)), (u32::from(r2 & 0x3FFF), u16::from(c2) | (r2 & 0xC000))))
        } else {
            let (r1, r2) = (self.row(c)?, self.row(c)?);
            let (c1, c2) = (c.u16()?, c.u16()?);
            Some(((r1, c1), (r2, c2)))
        }
    }

    /// A 3D reference's sheets: BIFF5 has the sheet indexes in the token.
    fn sheets3d(&self, c: &mut Cur) -> Option<u32> {
        if self.biff5 {
            let ixals = c.u16()? as i16;
            c.skip(8)?;
            let (a, b) = (c.u16()?, c.u16()?);
            (ixals <= 0).then_some(u32::from(a) | u32::from(b) << 16)
        } else {
            c.u16().map(u32::from)
        }
    }

    fn text(&self, c: &mut Cur, len16: bool) -> Option<String> {
        let n = if len16 { c.u16()? as usize } else { c.u8()? as usize };
        if self.biff5 {
            Some(c.bytes(n)?.iter().map(|&b| crate::ole::cp1252(b)).collect())
        } else {
            let wide = c.u8()? & 1 != 0;
            c.chars8(n, wide)
        }
    }

    /// A cell address: row, then column with its relative flags.
    fn cell(&self, row: u32, col: u16, rel: bool) -> (u32, u32, bool, bool) {
        let row_rel = col & 0x8000 != 0;
        let col_rel = col & 0x4000 != 0;
        let mut r = row;
        let mut c = u32::from(col & 0x3FFF);
        if rel {
            // Offsets from the base cell.
            if row_rel {
                let off = if self.biff12 {
                    row as i32 as i64
                } else if self.biff5 {
                    let v = i64::from(row & 0x3FFF);
                    if v >= 0x2000 { v - 0x4000 } else { v }
                } else {
                    row as u16 as i16 as i64
                };
                r = (self.base.row as i64 + off).rem_euclid(self.max_row() as i64 + 1) as u32;
            }
            if col_rel {
                let off = if self.biff12 {
                    let v = i64::from(col & 0x3FFF);
                    if v >= 0x2000 { v - 0x4000 } else { v }
                } else {
                    (col & 0xFF) as u8 as i8 as i64
                };
                c = (self.base.col as i64 + off).rem_euclid(self.max_col() as i64 + 1) as u32;
            }
        }
        (r, c, row_rel, col_rel)
    }

    /// In a name, a relative row or column past half the grid of the
    /// file points back (IV1 is the column to the left); written for the
    /// bigger grid, where XFD1 means the same.
    fn wrap(&self, (r, c, row_rel, col_rel): (u32, u32, bool, bool)) -> (u32, u32, bool, bool) {
        if !self.in_name || self.biff12 {
            return (r, c, row_rel, col_rel);
        }
        let (rows, cols) = (self.max_row() + 1, self.max_col() + 1);
        let r = if row_rel && r >= rows / 2 { r + 1_048_576 - rows } else { r };
        let c = if col_rel && c >= cols / 2 { c + 16_384 - cols } else { c };
        (r, c, row_rel, col_rel)
    }

    fn a1((r, c, row_rel, col_rel): (u32, u32, bool, bool)) -> String {
        format!("{}{}{}{}", if col_rel { "" } else { "$" }, col_letters(c), if row_rel { "" } else { "$" }, r + 1)
    }

    fn area(&self, a: (u32, u32, bool, bool), b: (u32, u32, bool, bool)) -> String {
        let d = |rel: bool| if rel { "" } else { "$" };
        let (wa, wb) = (self.wrap(a), self.wrap(b));
        if a.0 == 0 && b.0 == self.max_row() {
            format!("{}{}:{}{}", d(a.3), col_letters(wa.1), d(b.3), col_letters(wb.1))
        } else if a.1 == 0 && b.1 == self.max_col() {
            format!("{}{}:{}{}", d(a.2), wa.0 + 1, d(b.2), wb.0 + 1)
        } else {
            format!("{}:{}", Self::a1(wa), Self::a1(wb))
        }
    }

    fn prefix(&self, ixti: u32) -> Option<String> {
        (self.sheet)(ixti).map(|s| format!("{s}!"))
    }
}

fn quote_str(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn num_text(v: f64) -> String {
    let s = format!("{v}");
    if s.contains('e') || s.contains("inf") || s.contains("NaN") { format!("{v:E}") } else { s }
}

/// An array constant (`{1,2;3,4}`) from the formula's extra data.
fn read_array(x: &mut Cur, r: &Refs) -> Option<String> {
    let (cols, rows) = if r.biff12 {
        let rows = x.u32()? as usize;
        let cols = x.u32()? as usize;
        (cols, rows)
    } else {
        let cols = x.u8()? as usize + 1;
        let rows = x.u16()? as usize + 1;
        (cols, rows)
    };
    if cols.checked_mul(rows)? > 1_000_000 {
        return None;
    }
    let mut out = String::from("{");
    for i in 0..rows {
        if i > 0 {
            out.push(';');
        }
        for j in 0..cols {
            if j > 0 {
                out.push(',');
            }
            let ty = x.u8()?;
            if r.biff12 {
                match ty {
                    0 => out.push_str(&num_text(x.f64()?)),
                    1 => {
                        let n = x.u16()? as usize;
                        out.push_str(&quote_str(&x.utf16(n)?));
                    }
                    2 => out.push_str(if x.u8()? != 0 { "TRUE" } else { "FALSE" }),
                    4 => out.push_str(error_text(x.u8()?)),
                    _ => return None,
                }
            } else {
                match ty {
                    0 => x.skip(8)?,
                    1 => out.push_str(&num_text(x.f64()?)),
                    2 => out.push_str(&quote_str(&r.text(x, !r.biff5)?)),
                    4 => {
                        out.push_str(if x.u8()? != 0 { "TRUE" } else { "FALSE" });
                        x.skip(7)?;
                    }
                    16 => {
                        out.push_str(error_text(x.u8()?));
                        x.skip(7)?;
                    }
                    _ => return None,
                }
            }
        }
    }
    out.push('}');
    Some(out)
}

/// The formula text (without `=`) of parsed tokens `rgce` and their extra
/// data `rgcb`; None for what can't be read (the cell keeps its value).
/// Token bytes of long shared formulas (past 64) a sheet may copy into
/// its cells: many cells sharing a huge formula would make a tree each.
pub(crate) const SHARED_COPIES: usize = 8 << 20;

/// Charges one copy of a shared formula to `left`; false once spent.
pub(crate) fn charge_copy(left: &mut usize, rgce: &[u8], rgcb: &[u8]) -> bool {
    match left.checked_sub((rgce.len() + rgcb.len()).saturating_sub(64)) {
        Some(l) => {
            *left = l;
            true
        }
        None => false,
    }
}

pub(crate) fn decompile(rgce: &[u8], rgcb: &[u8], r: &Refs) -> Option<String> {
    let mut c = Cur::new(rgce);
    let mut x = Cur::new(rgcb);
    let mut st: Vec<String> = Vec::new();
    let bin = |st: &mut Vec<String>, op: &str| -> Option<()> {
        let b = st.pop()?;
        let a = st.last_mut()?;
        a.push_str(op);
        a.push_str(&b);
        Some(())
    };
    // What building the text may copy: linear in the tokens (operators
    // append; parentheses and signs wrapped again and again copy).
    let mut work = 0usize;
    let cap = rgce.len().saturating_mul(64).saturating_add(1 << 20);
    while c.left() > 0 {
        if work > cap {
            return None;
        }
        let depth = st.len();
        let top = st.last().map(|s| (s.as_ptr(), s.len()));
        let below = depth.checked_sub(2).map_or(0, |i| st[i].len());
        // Formulas are at most 8192 characters: one that grows past
        // (parentheses wrapped again and again) is not read.
        if st.len() > 1024 || st.last().is_some_and(|s| s.len() > 1 << 15) {
            return None;
        }
        let t = c.u8()?;
        // Reference tokens come in reference, value and array classes.
        let base = if t >= 0x20 { (t & 0x1F) | 0x20 } else { t };
        match base {
            0x03 => bin(&mut st, "+")?,
            0x04 => bin(&mut st, "-")?,
            0x05 => bin(&mut st, "*")?,
            0x06 => bin(&mut st, "/")?,
            0x07 => bin(&mut st, "^")?,
            0x08 => bin(&mut st, "&")?,
            0x09 => bin(&mut st, "<")?,
            0x0A => bin(&mut st, "<=")?,
            0x0B => bin(&mut st, "=")?,
            0x0C => bin(&mut st, ">=")?,
            0x0D => bin(&mut st, ">")?,
            0x0E => bin(&mut st, "<>")?,
            0x0F => bin(&mut st, " ")?,
            0x10 => bin(&mut st, ",")?,
            0x11 => bin(&mut st, ":")?,
            0x12 => {
                let a = st.pop()?;
                st.push(format!("+{a}"));
            }
            0x13 => {
                let a = st.pop()?;
                st.push(format!("-{a}"));
            }
            0x14 => {
                let a = st.pop()?;
                st.push(format!("{a}%"));
            }
            0x15 => {
                let a = st.pop()?;
                st.push(format!("({a})"));
            }
            0x16 => st.push(String::new()),
            0x17 => {
                let s = if r.biff12 {
                    let n = c.u16()? as usize;
                    c.utf16(n)?
                } else {
                    r.text(&mut c, false)?
                };
                st.push(quote_str(&s));
            }
            0x19 => {
                let flags = c.u8()?;
                let data = c.u16()?;
                if flags & 0x04 != 0 {
                    // CHOOSE's jump table.
                    c.skip((data as usize + 1) * 2)?;
                } else if flags & 0x10 != 0 {
                    let a = st.pop()?;
                    st.push(format!("SUM({a})"));
                }
            }
            0x1C => st.push(error_text(c.u8()?).to_string()),
            0x1D => st.push(if c.u8()? != 0 { "TRUE" } else { "FALSE" }.to_string()),
            0x1E => st.push(c.u16()?.to_string()),
            0x1F => st.push(num_text(c.f64()?)),
            0x20 => {
                c.skip(if r.biff12 { 14 } else { 7 })?;
                st.push(read_array(&mut x, r)?);
            }
            0x21 | 0x22 => {
                let (n, idx) = if base == 0x21 {
                    let idx = c.u16()?;
                    (fixed_args(idx)?, idx)
                } else {
                    let n = c.u8()? as usize & 0x7F;
                    (n, c.u16()? & 0x7FFF)
                };
                if st.len() < n {
                    return None;
                }
                let mut args = st.split_off(st.len() - n);
                let name = if idx == 255 {
                    if args.is_empty() {
                        return None;
                    }
                    args.remove(0)
                } else {
                    func_name(idx)?.to_string()
                };
                st.push(format!("{name}({})", args.join(",")));
            }
            0x23 => {
                // .xls: a 16-bit index and 2 unused bytes (BIFF5: 12).
                let i = c.u32()?;
                if r.biff5 {
                    c.skip(10)?;
                }
                st.push((r.name)(if r.biff12 { i } else { i & 0xFFFF })?);
            }
            0x24 | 0x2C => {
                let (row, col) = r.loc(&mut c)?;
                st.push(Refs::a1(r.wrap(r.cell(row, col, base == 0x2C))));
            }
            0x25 | 0x2D => {
                let ((r1, c1), (r2, c2)) = r.area_loc(&mut c)?;
                let rel = base == 0x2D;
                st.push(r.area(r.cell(r1, c1, rel), r.cell(r2, c2, rel)));
            }
            0x26..=0x28 => {
                // The subexpression that follows gives the value.
                c.skip(4)?;
                c.skip(2)?;
                if base == 0x26 {
                    let n = if r.biff12 {
                        x.u32()? as usize * 16
                    } else {
                        x.u16()? as usize * if r.biff5 { 6 } else { 8 }
                    };
                    x.skip(n)?;
                }
            }
            0x29 => c.skip(2)?,
            0x2A => {
                c.skip(if r.biff12 { 6 } else if r.biff5 { 3 } else { 4 })?;
                st.push("#REF!".into());
            }
            0x2B => {
                c.skip(if r.biff12 { 12 } else if r.biff5 { 6 } else { 8 })?;
                st.push("#REF!".into());
            }
            0x39 => {
                let ixti = c.u16()?;
                let i = if r.biff12 {
                    c.u32()?
                } else if r.biff5 {
                    c.skip(8)?;
                    let i = c.u16()?;
                    c.skip(12)?;
                    u32::from(i)
                } else {
                    let i = c.u16()?;
                    c.skip(2)?;
                    u32::from(i)
                };
                st.push((r.extern_name)(ixti, i)?);
            }
            0x3A => {
                let ixti = r.sheets3d(&mut c)?;
                let (row, col) = r.loc(&mut c)?;
                st.push(format!("{}{}", r.prefix(ixti)?, Refs::a1(r.wrap(r.cell(row, col, false)))));
            }
            0x3B => {
                let ixti = r.sheets3d(&mut c)?;
                let ((r1, c1), (r2, c2)) = r.area_loc(&mut c)?;
                st.push(format!("{}{}", r.prefix(ixti)?, r.area(r.cell(r1, c1, false), r.cell(r2, c2, false))));
            }
            0x3C => {
                c.skip(if r.biff12 { 8 } else if r.biff5 { 17 } else { 6 })?;
                st.push("#REF!".into());
            }
            0x3D => {
                c.skip(if r.biff12 { 14 } else if r.biff5 { 20 } else { 10 })?;
                st.push("#REF!".into());
            }
            // Shared and table formulas (handled by the caller), structured
            // references and anything else.
            _ => return None,
        }
        if (st.len(), st.last().map(|s| (s.as_ptr(), s.len()))) != (depth, top) {
            let made = st.last().map_or(0, String::len);
            // An operator copies only its right side.
            work += if (0x03..=0x11).contains(&base) { made.saturating_sub(below) } else { made };
        }
    }
    if st.len() == 1 { st.pop() } else { None }
}

/// Tokens made by hand, for tests.
#[cfg(test)]
pub(crate) fn tokens(parts: &[&[u8]]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in parts {
        v.extend_from_slice(p);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs<'a>(biff12: bool, sheet: &'a dyn Fn(u32) -> Option<String>, name: &'a dyn Fn(u32) -> Option<String>, ext: &'a dyn Fn(u16, u32) -> Option<String>) -> Refs<'a> {
        Refs { biff12, biff5: false, base: CellRef::new(4, 2), in_name: false, sheet, name, extern_name: ext }
    }

    #[test]
    fn function_table() {
        assert_eq!(FUNCS.len(), 485);
        assert_eq!(func_name(4), Some("SUM"));
        assert_eq!(func_name(102), Some("VLOOKUP"));
        assert_eq!(func_name(221), Some("TODAY"));
        assert_eq!(func_name(336), Some("CONCATENATE"));
        assert_eq!(func_name(368), Some("BAHTTEXT"));
        assert_eq!(func_name(480), Some("IFERROR"));
        assert_eq!(func_name(484), Some("AVERAGEIFS"));
    }

    #[test]
    fn decompiles_biff8() {
        let sheet = |i: u32| (i == 0).then(|| quote_sheet("Hoja 2"));
        let name = |i: u32| (i == 1).then(|| "Tasa".to_string());
        let ext = |_: u16, i: u32| (i == 1).then(|| "_xlfn.XLOOKUP".to_string());
        let r = refs(false, &sheet, &name, &ext);
        // SUM(A1:B2)*2+$C$3
        let f = tokens(&[&[0x25, 0, 0, 1, 0, 0, 0xC0, 1, 0xC0], &[0x22, 1, 4, 0], &[0x1E, 2, 0], &[0x05], &[0x24, 2, 0, 2, 0], &[0x03]]);
        assert_eq!(decompile(&f, &[], &r).as_deref(), Some("SUM(A1:B2)*2+$C$3"));
        // IF(A1>0,"sí",Tasa) with the IF jumps.
        let f = tokens(&[&[0x44, 0, 0, 0, 0xC0], &[0x1E, 0, 0], &[0x0D], &[0x19, 0x02, 0, 0], &[0x17, 2, 0, b's', 0xED], &[0x19, 0x08, 0, 0], &[0x43, 1, 0, 0, 0], &[0x19, 0x08, 0, 0], &[0x42, 3, 1, 0]]);
        assert_eq!(decompile(&f, &[], &r).as_deref(), Some("IF(A1>0,\"sí\",Tasa)"));
        // 'Hoja 2'!A:A and a relative reference of a shared formula (row
        // -1, column +1 from C5).
        let f = tokens(&[&[0x3B, 0, 0, 0, 0, 0xFF, 0xFF, 0, 0x80, 0, 0x80], &[0x2C, 0xFF, 0xFF, 1, 0xC0], &[0x03]]);
        assert_eq!(decompile(&f, &[], &r).as_deref(), Some("'Hoja 2'!$A:$A+D4"));
        // An add-in function: name first, then the arguments.
        let f = tokens(&[&[0x39, 0, 0, 1, 0, 0, 0], &[0x1E, 1, 0], &[0x22, 2, 0xFF, 0]]);
        assert_eq!(decompile(&f, &[], &r).as_deref(), Some("_xlfn.XLOOKUP(1)"));
        // {1,"a";TRUE,#N/A}
        let f = [0x60, 0, 0, 0, 0, 0, 0, 0];
        let mut extra = vec![1, 1, 0];
        extra.push(1);
        extra.extend_from_slice(&1f64.to_le_bytes());
        extra.extend_from_slice(&[2, 1, 0, 0, b'a']);
        extra.extend_from_slice(&[4, 1, 0, 0, 0, 0, 0, 0, 0]);
        extra.extend_from_slice(&[16, 0x2A, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(decompile(&f, &extra, &r).as_deref(), Some("{1,\"a\";TRUE,#N/A}"));
        // A lone missing function argument and a percentage.
        let f = tokens(&[&[0x1F], &0.5f64.to_le_bytes(), &[0x14]]);
        assert_eq!(decompile(&f, &[], &r).as_deref(), Some("0.5%"));
    }

    #[test]
    fn decompiles_biff12() {
        let sheet = |_: u32| Some("Datos".to_string());
        let name = |_: u32| None;
        let ext = |_: u16, _: u32| None;
        let r = refs(true, &sheet, &name, &ext);
        // Datos!B2 & "x"
        let f = tokens(&[&[0x3A, 0, 0, 1, 0, 0, 0, 1, 0], &[0x17, 1, 0, b'x', 0], &[0x08]]);
        assert_eq!(decompile(&f, &[], &r).as_deref(), Some("Datos!$B$2&\"x\""));
        // A1:XFD1 is the whole row.
        let f = tokens(&[&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xC0, 0xFF, 0xFF]]);
        assert_eq!(decompile(&f, &[], &r).as_deref(), Some("1:1"));
    }

    #[test]
    fn decompiles_biff5() {
        let sheet = |i: u32| (i == 0x0001_0001).then(|| "Hoja2".to_string());
        let name = |_: u32| None;
        let ext = |_: u16, _: u32| None;
        let mut r = refs(false, &sheet, &name, &ext);
        r.biff5 = true;
        // $A$1+B2 (relative), "é" and Hoja2!C3.
        let f = tokens(&[&[0x24, 0, 0, 0], &[0x24, 1, 0xC0, 1], &[0x03], &[0x17, 1, 0xE9], &[0x08], &[0x3A, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 2, 0, 2], &[0x08]]);
        assert_eq!(decompile(&f, &[], &r).as_deref(), Some("$A$1+B2&\"é\"&Hoja2!$C$3"));
    }
}
