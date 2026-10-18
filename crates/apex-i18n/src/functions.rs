//! Spreadsheet functions in the interface language: categories, the
//! names shown (Spanish aliases in Spanish), argument help and
//! autocomplete. Shared by the window and the terminal interface.

use apex_sheet::functions::Func;

use crate as i18n;

pub const CATEGORIES: [&str; 12] = ["fncat.math", "fncat.stat", "fncat.logic", "fncat.lookup", "fncat.text", "fncat.info", "fncat.date", "fncat.finance", "fncat.eng", "fncat.db", "fncat.compat", "fncat.web"];

pub fn category(f: Func) -> &'static str {
    use Func::*;
    match f {
        SumSq | SumX2My2 | SumX2Py2 | SumXMy2 | Gcd | Lcm | Fact | FactDouble | Combin | Combina | Permut | Permutationa | Multinomial | Sin | Cos | Tan | Asin | Acos | Atan | Atan2 | Sinh | Cosh | Tanh | Asinh | Acosh | Atanh | Cot | Coth | Csc | Csch | Sec | Sech | Acot | Acoth | Degrees | Radians | Roman | Arabic | Base | Decimal | MRound | Even | Odd | Quotient | SqrtPi | CeilingMath | FloorMath | CeilingPrecise | FloorPrecise | Aggregate | SeriesSum | MMult | MInverse | MDeterm | MUnit | Sequence | RandArray => "fncat.math",
        AverageA | MaxA | MinA | AverageIfs | MaxIfs | MinIfs | Mode | ModeMult | Percentile | PercentileExc | Quartile | QuartileExc | PercentRank | PercentRankExc | RankAvg | StdevP | StdevA | StdevPA | VarP | VarA | VarPA | AveDev | DevSq | GeoMean | HarMean | TrimMean | Kurt | Skew | SkewP | Correl | Pearson | Rsq | Covar | CovarianceS | Slope | Intercept | Steyx | Forecast | ForecastEts | ForecastEtsConfint | ForecastEtsSeasonality | ForecastEtsStat | Trend | Growth | Linest | Logest | Frequency | Standardize | NormDist | NormInv | NormSDist | NormSInv | LognormDist | LognormInv | TDist | TDist2T | TDistRT | TInv | TInv2T | TTest | ChisqDist | ChisqDistRT | ChisqInv | ChisqInvRT | ChisqTest | FDist | FDistRT | FInv | FInvRT | FTest | BinomDist | BinomInv | PoissonDist | ExponDist | GammaDist | GammaInv | Gamma | GammaLn | BetaDist | BetaInv | WeibullDist | HypgeomDist | NegbinomDist | ConfidenceNorm | ConfidenceT | ZTest | Fisher | FisherInv | Prob | Phi | Gauss => "fncat.stat",
        N | IsEven | IsOdd | Type | ErrorType | CellInfo | IsLogical | IsNonText | IsRef | IsErr | IsFormula | SheetNum | SheetsNum => "fncat.info",
        NetworkDays | NetworkDaysIntl | Workday | WorkdayIntl | DateDif | WeekNum | IsoWeekNum | YearFrac | Time | DateValue | TimeValue | Days360 => "fncat.date",
        Rate | Nper | Irr | Xirr | Xnpv | Ipmt | Ppmt | Sln | Syd | Db | Ddb | Vdb | Mirr | Effect | Nominal | CumIpmt | CumPrinc | IsPmt | Rri | PDuration | FvSchedule | DollarDe | DollarFr => "fncat.finance",
        Bin2Dec | Bin2Hex | Bin2Oct | Dec2Bin | Dec2Hex | Dec2Oct | Hex2Bin | Hex2Dec | Hex2Oct | Oct2Bin | Oct2Dec | Oct2Hex | Convert | Delta | GeStep | BitAnd | BitOr | BitXor | BitLShift | BitRShift | Erf | Erfc => "fncat.eng",
        Lookup | Offset | Indirect | Address | XMatch | ChooseCols | ChooseRows | Take | Drop | Filter | Sort | SortBy | Unique | VStack | HStack | Transpose | Expand | ToCol | ToRow | WrapRows | WrapCols | Areas | FormulaText | Hyperlink | Image => "fncat.lookup",
        Let | Lambda | Map | Reduce | Scan | ByRow | ByCol | MakeArray | IsOmitted | Switch => "fncat.logic",
        Sum | Product | Abs | Round | RoundUp | RoundDown | Int | Trunc | Mod | Power | Sqrt | Exp | Ln | Log | Log10 | Pi | Sign | Ceiling | Floor | Rand | RandBetween | SumIf | SumIfs | SumProduct | Subtotal => "fncat.math",
        Average | AverageIf | Min | Max | Count | CountA | CountBlank | CountIf | CountIfs | Median | Stdev | Var | Large | Small | Rank => "fncat.stat",
        If | IfError | IfNa | Ifs | And | Or | Not | Xor | True | False => "fncat.logic",
        VLookup | HLookup | XLookup | Index | Match | Choose | Row | Column | Rows | Columns => "fncat.lookup",
        IsBlank | IsNumber | IsText | IsError | IsNa | Na => "fncat.info",
        Today | Now | Date | Year | Month | Day | Hour | Minute | Second | Weekday | EDate | EoMonth | Days => "fncat.date",
        Pmt | Fv | Pv | Npv => "fncat.finance",
        EncodeUrl | FilterXml => "fncat.web",
        Info => "fncat.info",
        Complex | ImAbs | Imaginary | ImArgument | ImConjugate | ImCos | ImCosh | ImCot | ImCsc | ImCsch | ImDiv | ImExp | ImLn | ImLog10 | ImLog2 | ImPower | ImProduct | ImReal | ImSec | ImSech | ImSin | ImSinh | ImSqrt | ImSub | ImSum | ImTan | BesselI | BesselJ | BesselK | BesselY => "fncat.eng",
        EcmaCeiling => "fncat.math",
        NormDistLegacy | NormInvLegacy | NormSDistLegacy | NormSInvLegacy | TDistLegacy | BetaDistLegacy | BetaInvLegacy | HypgeomDistLegacy | LognormDistLegacy => "fncat.compat",
        DAverage | DCount | DCountA | DGet | DMax | DMin | DProduct | DStdev | DStdevP | DSum | DVar | DVarP => "fncat.db",
        GroupBy | PivotBy | TrimRange | GetPivotData => "fncat.lookup",
        PercentOf => "fncat.math",
        BinomDistRange => "fncat.stat",
        UsDollar => "fncat.text",
        EuroConvert => "fncat.finance",
        Query | ArrayFormula | Flatten | ArrayConstrain | SortN => "fncat.lookup",
        GAdd | GMinus | GMultiply | GDivide | GPow | GUMinus | GUPlus | GUnaryPercent => "fncat.math",
        GEq | GNe | GGt | GGte | GLt | GLte => "fncat.logic",
        CountUnique => "fncat.stat",
        IsEmail | IsUrl => "fncat.info",
        AccrInt | AccrIntM | AmorDegrc | AmorLinc | CoupDayBs | CoupDays | CoupDaysNc | CoupNcd | CoupNum | CoupPcd | Disc | Duration | IntRate | MDuration | OddFPrice | OddFYield | OddLPrice | OddLYield | Price | PriceDisc | PriceMat | Received | TBillEq | TBillPrice | TBillYield | Yield | YieldDisc | YieldMat => "fncat.finance",
        _ => "fncat.text",
    }
}

/// The name shown to the user: the Spanish alias in Spanish, else English.
pub fn display_name(f: Func) -> &'static str {
    if i18n::current() == "es" {
        if let Some(a) = f.aliases().iter().find(|a| !a.contains(".S") && !a.starts_with("RANK") && **a != "JIS") {
            return a;
        }
    }
    f.name()
}

/// Arguments (with the locale separator) and description.
pub fn help(f: Func, sep: char) -> (String, &'static str) {
    let text = i18n::lookup(&format!("fn.{}", f.name())).unwrap_or("");
    let (args, desc) = text.split_once('|').unwrap_or(("", text));
    let args = args.trim().replace(',', &sep.to_string());
    (args, desc.trim())
}

/// Functions whose name (or alias) starts with `prefix`, with the name that
/// matched. Exact matches and the localized names come first.
pub fn complete(prefix: &str) -> Vec<(&'static str, Func)> {
    let p = prefix.to_uppercase();
    if p.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(&'static str, Func, bool)> = Vec::new();
    for &f in Func::ALL {
        let shown = display_name(f);
        if let Some(n) = [shown, f.name()].into_iter().find(|n| n.starts_with(&p)) {
            out.push((n, f, false));
        } else if let Some(n) = f.aliases().iter().copied().find(|n| n.starts_with(&p)) {
            out.push((n, f, true));
        }
    }
    // Aliases (other languages' names) only after every real name.
    out.sort_by_key(|&(n, _, alias)| (n.len() != p.len(), alias, n));
    out.into_iter().map(|(n, f, _)| (n, f)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_and_has_help() {
        assert!(complete("su").iter().any(|(_, f)| *f == Func::Sum));
        // Aliases follow the real names: "B" lists BASE before BDSUMA.
        let b = complete("b");
        let first_alias = b.iter().position(|(n, _)| *n == "BDSUMA").unwrap();
        assert!(b.iter().position(|(n, _)| *n == "BASE").unwrap() < first_alias);
        assert!(b[first_alias..].iter().all(|(n, f)| f.aliases().contains(n)));
        for &f in Func::ALL {
            assert!(i18n::lookup(&format!("fn.{}", f.name())).is_some(), "missing help for {}", f.name());
        }
    }
}
