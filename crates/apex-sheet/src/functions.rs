//! Built-in function catalogue and evaluator.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::sync::Arc;

use crate::addr::{CellRef, MAX_COLS, MAX_ROWS, Range};
use crate::format::{self, Locale, NumFormat};
use crate::formula::{BinOp, CellAddr, Expr, RangeKind, UnOp};
use crate::value::{ErrorKind, Lambda, Matrix, Value, cmp_text_ci};

macro_rules! funcs {
    ($( $v:ident = $name:literal $(| $alias:literal)* ),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Func { $($v),* }

        impl Func {
            pub const ALL: &'static [Func] = &[$(Func::$v),*];

            pub fn name(self) -> &'static str {
                match self { $(Func::$v => $name),* }
            }

            /// Accepts English names and Spanish aliases, case-insensitive.
            pub fn lookup(s: &str) -> Option<Func> {
                let u = s.to_uppercase();
                let u = u.strip_prefix("_XLFN.").unwrap_or(&u);
                match u {
                    $($name $(| $alias)* => Some(Func::$v),)*
                    _ => None,
                }
            }

            /// Localized aliases (for autocomplete).
            pub fn aliases(self) -> &'static [&'static str] {
                match self { $(Func::$v => &[$($alias),*]),* }
            }
        }
    };
}

funcs! {
    Sum = "SUM" | "SUMA",
    Average = "AVERAGE" | "PROMEDIO",
    Min = "MIN",
    Max = "MAX",
    Count = "COUNT" | "CONTAR",
    CountA = "COUNTA" | "CONTARA",
    CountBlank = "COUNTBLANK" | "CONTAR.BLANCO",
    Product = "PRODUCT" | "PRODUCTO",
    Median = "MEDIAN" | "MEDIANA",
    Stdev = "STDEV" | "DESVEST" | "STDEV.S" | "DESVEST.M",
    Var = "VAR" | "VAR.S",
    If = "IF" | "SI",
    IfError = "IFERROR" | "SI.ERROR",
    IfNa = "IFNA" | "SI.ND",
    Ifs = "IFS" | "SI.CONJUNTO",
    And = "AND" | "Y",
    Or = "OR" | "O",
    Not = "NOT" | "NO",
    Xor = "XOR",
    True = "TRUE" | "VERDADERO",
    False = "FALSE" | "FALSO",
    Abs = "ABS",
    Round = "ROUND" | "REDONDEAR",
    RoundUp = "ROUNDUP" | "REDONDEAR.MAS",
    RoundDown = "ROUNDDOWN" | "REDONDEAR.MENOS",
    Int = "INT" | "ENTERO",
    Trunc = "TRUNC" | "TRUNCAR",
    Mod = "MOD" | "RESIDUO",
    Power = "POWER" | "POTENCIA",
    Sqrt = "SQRT" | "RAIZ",
    Exp = "EXP",
    Ln = "LN",
    Log = "LOG",
    Log10 = "LOG10",
    Pi = "PI",
    Sign = "SIGN" | "SIGNO",
    Ceiling = "CEILING" | "MULTIPLO.SUPERIOR",
    Floor = "FLOOR" | "MULTIPLO.INFERIOR",
    Rand = "RAND" | "ALEATORIO",
    RandBetween = "RANDBETWEEN" | "ALEATORIO.ENTRE",
    SumIf = "SUMIF" | "SUMAR.SI",
    SumIfs = "SUMIFS" | "SUMAR.SI.CONJUNTO",
    CountIf = "COUNTIF" | "CONTAR.SI",
    CountIfs = "COUNTIFS" | "CONTAR.SI.CONJUNTO",
    AverageIf = "AVERAGEIF" | "PROMEDIO.SI",
    SumProduct = "SUMPRODUCT" | "SUMAPRODUCTO",
    VLookup = "VLOOKUP" | "BUSCARV",
    HLookup = "HLOOKUP" | "BUSCARH",
    XLookup = "XLOOKUP" | "BUSCARX",
    Index = "INDEX" | "INDICE",
    Match = "MATCH" | "COINCIDIR",
    Choose = "CHOOSE" | "ELEGIR",
    Row = "ROW" | "FILA",
    Column = "COLUMN" | "COLUMNA",
    Rows = "ROWS" | "FILAS",
    Columns = "COLUMNS" | "COLUMNAS",
    Concatenate = "CONCATENATE" | "CONCATENAR",
    Concat = "CONCAT",
    TextJoin = "TEXTJOIN" | "UNIRCADENAS",
    Len = "LEN" | "LARGO",
    Upper = "UPPER" | "MAYUSC",
    Lower = "LOWER" | "MINUSC",
    Proper = "PROPER" | "NOMPROPIO",
    Trim = "TRIM" | "ESPACIOS",
    Left = "LEFT" | "IZQUIERDA",
    Right = "RIGHT" | "DERECHA",
    Mid = "MID" | "EXTRAE",
    Find = "FIND" | "ENCONTRAR",
    Search = "SEARCH" | "HALLAR",
    Substitute = "SUBSTITUTE" | "SUSTITUIR",
    Replace = "REPLACE" | "REEMPLAZAR",
    Rept = "REPT" | "REPETIR",
    Exact = "EXACT" | "IGUAL",
    Text = "TEXT" | "TEXTO",
    ValueFn = "VALUE" | "VALOR",
    Char = "CHAR" | "CARACTER",
    Code = "CODE" | "CODIGO",
    IsBlank = "ISBLANK" | "ESBLANCO",
    IsNumber = "ISNUMBER" | "ESNUMERO",
    IsText = "ISTEXT" | "ESTEXTO",
    IsError = "ISERROR" | "ESERROR",
    IsNa = "ISNA" | "ESNOD",
    Na = "NA" | "NOD",
    Today = "TODAY" | "HOY",
    Now = "NOW" | "AHORA",
    Date = "DATE" | "FECHA",
    Year = "YEAR" | "AÑO",
    Month = "MONTH" | "MES",
    Day = "DAY" | "DIA",
    Hour = "HOUR" | "HORA",
    Minute = "MINUTE" | "MINUTO",
    Second = "SECOND" | "SEGUNDO",
    Weekday = "WEEKDAY" | "DIASEM",
    EDate = "EDATE" | "FECHA.MES",
    EoMonth = "EOMONTH" | "FIN.MES",
    Days = "DAYS" | "DIAS",
    Pmt = "PMT" | "PAGO",
    Fv = "FV" | "VF",
    Pv = "PV" | "VA",
    Npv = "NPV" | "VNA",
    Large = "LARGE" | "K.ESIMO.MAYOR",
    Small = "SMALL" | "K.ESIMO.MENOR",
    Rank = "RANK" | "JERARQUIA" | "RANK.EQ",
    Subtotal = "SUBTOTAL" | "SUBTOTALES",
    // Math and trigonometry
    SumSq = "SUMSQ" | "SUMA.CUADRADOS",
    SumX2My2 = "SUMX2MY2" | "SUMAX2MENOSY2",
    SumX2Py2 = "SUMX2PY2" | "SUMAX2MASY2",
    SumXMy2 = "SUMXMY2" | "SUMAXMENOSY2",
    Gcd = "GCD" | "M.C.D",
    Lcm = "LCM" | "M.C.M",
    Fact = "FACT",
    FactDouble = "FACTDOUBLE" | "FACT.DOBLE",
    Combin = "COMBIN" | "COMBINAT",
    Combina = "COMBINA",
    Permut = "PERMUT" | "PERMUTACIONES",
    Permutationa = "PERMUTATIONA" | "PERMUTACIONES.A",
    Multinomial = "MULTINOMIAL",
    Sin = "SIN" | "SENO",
    Cos = "COS",
    Tan = "TAN",
    Asin = "ASIN" | "ASENO",
    Acos = "ACOS",
    Atan = "ATAN",
    Atan2 = "ATAN2",
    Sinh = "SINH" | "SENOH",
    Cosh = "COSH",
    Tanh = "TANH",
    Asinh = "ASINH" | "ASENOH",
    Acosh = "ACOSH",
    Atanh = "ATANH",
    Cot = "COT",
    Coth = "COTH",
    Csc = "CSC",
    Csch = "CSCH",
    Sec = "SEC",
    Sech = "SECH",
    Acot = "ACOT",
    Acoth = "ACOTH",
    Degrees = "DEGREES" | "GRADOS",
    Radians = "RADIANS" | "RADIANES",
    Roman = "ROMAN" | "NUMERO.ROMANO",
    Arabic = "ARABIC" | "NUMERO.ARABE",
    Base = "BASE",
    Decimal = "DECIMAL" | "CONV.DECIMAL",
    MRound = "MROUND" | "REDOND.MULT",
    Even = "EVEN" | "REDONDEA.PAR",
    Odd = "ODD" | "REDONDEA.IMPAR",
    Quotient = "QUOTIENT" | "COCIENTE",
    SqrtPi = "SQRTPI" | "RAIZ2PI",
    CeilingMath = "CEILING.MATH" | "MULTIPLO.SUPERIOR.MAT",
    FloorMath = "FLOOR.MATH" | "MULTIPLO.INFERIOR.MAT",
    CeilingPrecise = "CEILING.PRECISE" | "MULTIPLO.SUPERIOR.EXACTO" | "ISO.CEILING" | "MULTIPLO.SUPERIOR.ISO",
    FloorPrecise = "FLOOR.PRECISE" | "MULTIPLO.INFERIOR.EXACTO",
    Aggregate = "AGGREGATE" | "AGREGAR",
    SeriesSum = "SERIESSUM" | "SUMA.SERIES",
    MMult = "MMULT",
    MInverse = "MINVERSE" | "MINVERSA",
    MDeterm = "MDETERM",
    MUnit = "MUNIT" | "M.UNIDAD",
    Sequence = "SEQUENCE" | "SECUENCIA",
    RandArray = "RANDARRAY" | "MATRIZALEAT",
    // Statistics
    AverageA = "AVERAGEA" | "PROMEDIOA",
    MaxA = "MAXA",
    MinA = "MINA",
    AverageIfs = "AVERAGEIFS" | "PROMEDIO.SI.CONJUNTO",
    MaxIfs = "MAXIFS" | "MAX.SI.CONJUNTO",
    MinIfs = "MINIFS" | "MIN.SI.CONJUNTO",
    Mode = "MODE" | "MODA" | "MODE.SNGL" | "MODA.UNO",
    ModeMult = "MODE.MULT" | "MODA.VARIOS",
    Percentile = "PERCENTILE" | "PERCENTIL" | "PERCENTILE.INC" | "PERCENTIL.INC",
    PercentileExc = "PERCENTILE.EXC" | "PERCENTIL.EXC",
    Quartile = "QUARTILE" | "CUARTIL" | "QUARTILE.INC" | "CUARTIL.INC",
    QuartileExc = "QUARTILE.EXC" | "CUARTIL.EXC",
    PercentRank = "PERCENTRANK" | "RANGO.PERCENTIL" | "PERCENTRANK.INC" | "RANGO.PERCENTIL.INC",
    PercentRankExc = "PERCENTRANK.EXC" | "RANGO.PERCENTIL.EXC",
    RankAvg = "RANK.AVG" | "JERARQUIA.MEDIA",
    StdevP = "STDEVP" | "DESVESTP" | "STDEV.P" | "DESVEST.P",
    StdevA = "STDEVA" | "DESVESTA",
    StdevPA = "STDEVPA" | "DESVESTPA",
    VarP = "VARP" | "VAR.P",
    VarA = "VARA",
    VarPA = "VARPA",
    AveDev = "AVEDEV" | "DESVPROM",
    DevSq = "DEVSQ" | "DESVIA2",
    GeoMean = "GEOMEAN" | "MEDIA.GEOM",
    HarMean = "HARMEAN" | "MEDIA.ARMO",
    TrimMean = "TRIMMEAN" | "MEDIA.ACOTADA",
    Kurt = "KURT" | "CURTOSIS",
    Skew = "SKEW" | "COEFICIENTE.ASIMETRIA",
    SkewP = "SKEW.P" | "COEFICIENTE.ASIMETRIA.P",
    Correl = "CORREL" | "COEF.DE.CORREL",
    Pearson = "PEARSON",
    Rsq = "RSQ" | "COEFICIENTE.R2",
    Covar = "COVAR" | "COVARIANZA.P" | "COVARIANCE.P",
    CovarianceS = "COVARIANCE.S" | "COVARIANZA.M",
    Slope = "SLOPE" | "PENDIENTE",
    Intercept = "INTERCEPT" | "INTERSECCION.EJE",
    Steyx = "STEYX" | "ERROR.TIPICO.XY",
    Forecast = "FORECAST" | "PRONOSTICO" | "FORECAST.LINEAR" | "PRONOSTICO.LINEAL",
    // LibreOffice's names (additive versions) are read as these.
    ForecastEts = "FORECAST.ETS" | "PRONOSTICO.ETS" | "FORECAST.ETS.ADD" | "ORG.LIBREOFFICE.FORECAST.ETS.ADD",
    ForecastEtsConfint = "FORECAST.ETS.CONFINT" | "PRONOSTICO.ETS.CONFINT" | "FORECAST.ETS.PI.ADD" | "ORG.LIBREOFFICE.FORECAST.ETS.PI.ADD",
    ForecastEtsSeasonality = "FORECAST.ETS.SEASONALITY" | "PRONOSTICO.ETS.ESTACIONALIDAD" | "ORG.LIBREOFFICE.FORECAST.ETS.SEASONALITY",
    ForecastEtsStat = "FORECAST.ETS.STAT" | "PRONOSTICO.ETS.ESTADISTICA" | "FORECAST.ETS.STAT.ADD" | "ORG.LIBREOFFICE.FORECAST.ETS.STAT.ADD",
    Trend = "TREND" | "TENDENCIA",
    Growth = "GROWTH" | "CRECIMIENTO",
    Linest = "LINEST" | "ESTIMACION.LINEAL",
    Logest = "LOGEST" | "ESTIMACION.LOGARITMICA",
    Frequency = "FREQUENCY" | "FRECUENCIA",
    Standardize = "STANDARDIZE" | "NORMALIZACION",
    NormDist = "NORM.DIST" | "DISTR.NORM.N",
    NormInv = "NORM.INV" | "INV.NORM",
    NormSDist = "NORM.S.DIST" | "DISTR.NORM.ESTAND.N",
    NormSInv = "NORM.S.INV" | "INV.NORM.ESTAND",
    NormDistLegacy = "NORMDIST" | "DISTR.NORM",
    NormInvLegacy = "NORMINV" | "DISTR.NORM.INV",
    NormSDistLegacy = "NORMSDIST" | "DISTR.NORM.ESTAND",
    NormSInvLegacy = "NORMSINV" | "DISTR.NORM.ESTAND.INV",
    LognormDist = "LOGNORM.DIST" | "DISTR.LOGNORM",
    LognormInv = "LOGNORM.INV" | "INV.LOGNORM" | "LOGINV" | "DISTR.LOG.INV",
    TDist = "T.DIST" | "DISTR.T.N",
    TDist2T = "T.DIST.2T" | "DISTR.T.2C",
    TDistRT = "T.DIST.RT" | "DISTR.T.CD",
    TDistLegacy = "TDIST" | "DISTR.T",
    TInv = "T.INV" | "INV.T",
    TInv2T = "T.INV.2T" | "INV.T.2C" | "TINV" | "DISTR.T.INV",
    TTest = "T.TEST" | "PRUEBA.T.N" | "TTEST" | "PRUEBA.T",
    ChisqDist = "CHISQ.DIST" | "DISTR.CHICUAD",
    ChisqDistRT = "CHISQ.DIST.RT" | "DISTR.CHICUAD.CD" | "CHIDIST" | "DISTR.CHI",
    ChisqInv = "CHISQ.INV" | "INV.CHICUAD",
    ChisqInvRT = "CHISQ.INV.RT" | "INV.CHICUAD.CD" | "CHIINV" | "PRUEBA.CHI.INV",
    ChisqTest = "CHISQ.TEST" | "PRUEBA.CHICUAD" | "CHITEST" | "PRUEBA.CHI",
    FDist = "F.DIST" | "DISTR.F.N",
    FDistRT = "F.DIST.RT" | "DISTR.F.CD" | "FDIST" | "DISTR.F",
    FInv = "F.INV" | "INV.F",
    FInvRT = "F.INV.RT" | "INV.F.CD" | "FINV" | "DISTR.F.INV",
    FTest = "F.TEST" | "PRUEBA.F.N" | "FTEST" | "PRUEBA.F",
    BinomDist = "BINOM.DIST" | "DISTR.BINOM.N" | "BINOMDIST" | "DISTR.BINOM",
    BinomInv = "BINOM.INV" | "INV.BINOM" | "CRITBINOM" | "BINOM.CRIT",
    BinomDistRange = "BINOM.DIST.RANGE" | "DISTR.BINOM.SERIE",
    PoissonDist = "POISSON.DIST" | "POISSON",
    ExponDist = "EXPON.DIST" | "DISTR.EXP.N" | "EXPONDIST" | "DISTR.EXP",
    GammaDist = "GAMMA.DIST" | "DISTR.GAMMA" | "GAMMADIST",
    GammaInv = "GAMMA.INV" | "INV.GAMMA" | "GAMMAINV" | "DISTR.GAMMA.INV",
    Gamma = "GAMMA",
    GammaLn = "GAMMALN" | "GAMMA.LN" | "GAMMALN.PRECISE" | "GAMMA.LN.EXACTO",
    BetaDist = "BETA.DIST" | "DISTR.BETA.N",
    BetaInv = "BETA.INV" | "INV.BETA.N",
    WeibullDist = "WEIBULL.DIST" | "DISTR.WEIBULL" | "WEIBULL",
    HypgeomDist = "HYPGEOM.DIST" | "DISTR.HIPERGEOM.N",
    NegbinomDist = "NEGBINOM.DIST" | "NEGBINOMDIST",
    ConfidenceNorm = "CONFIDENCE.NORM" | "INTERVALO.CONFIANZA.NORM" | "CONFIDENCE" | "INTERVALO.CONFIANZA",
    ConfidenceT = "CONFIDENCE.T" | "INTERVALO.CONFIANZA.T",
    ZTest = "Z.TEST" | "PRUEBA.Z.N" | "ZTEST" | "PRUEBA.Z",
    Fisher = "FISHER",
    FisherInv = "FISHERINV" | "PRUEBA.FISHER.INV",
    Prob = "PROB" | "PROBABILIDAD",
    Phi = "PHI" | "FI",
    Gauss = "GAUSS",
    // Text
    TextBefore = "TEXTBEFORE" | "TEXTOANTES",
    TextAfter = "TEXTAFTER" | "TEXTODESPUES",
    TextSplit = "TEXTSPLIT" | "DIVIDIRTEXTO",
    RegexTest = "REGEXTEST" | "REGEXMATCH",
    RegexExtract = "REGEXEXTRACT",
    RegexReplace = "REGEXREPLACE",
    UniChar = "UNICHAR" | "UNICAR",
    Unicode = "UNICODE",
    Clean = "CLEAN" | "LIMPIAR",
    NumberValue = "NUMBERVALUE" | "VALOR.NUMERO",
    Fixed = "FIXED",
    Dollar = "DOLLAR" | "MONEDA",
    // DOLLAR always in US dollars, whatever the locale.
    UsDollar = "USDOLLAR",
    T = "T",
    N = "N",
    ValueToText = "VALUETOTEXT" | "VALORATEXTO",
    ArrayToText = "ARRAYTOTEXT" | "MATRIZATEXTO",
    LenB = "LENB" | "LARGOB",
    LeftB = "LEFTB" | "IZQUIERDAB",
    RightB = "RIGHTB" | "DERECHAB",
    MidB = "MIDB" | "EXTRAEB",
    FindB = "FINDB" | "ENCONTRARB",
    SearchB = "SEARCHB" | "HALLARB",
    ReplaceB = "REPLACEB" | "REEMPLAZARB",
    Asc = "ASC",
    // LibreOffice calls it JIS.
    Dbcs = "DBCS" | "JIS",
    Phonetic = "PHONETIC" | "FONETICO",
    BahtText = "BAHTTEXT" | "TEXTOBAHT",
    EncodeUrl = "ENCODEURL" | "URLCODIF",
    FilterXml = "FILTERXML" | "XMLFILTRO",
    Info = "INFO",
    Image = "IMAGE" | "IMAGEN",
    // Date and time
    NetworkDays = "NETWORKDAYS" | "DIAS.LAB",
    NetworkDaysIntl = "NETWORKDAYS.INTL" | "DIAS.LAB.INTL",
    Workday = "WORKDAY" | "DIA.LAB",
    WorkdayIntl = "WORKDAY.INTL" | "DIA.LAB.INTL",
    DateDif = "DATEDIF" | "SIFECHA",
    WeekNum = "WEEKNUM" | "NUM.DE.SEMANA",
    IsoWeekNum = "ISOWEEKNUM" | "ISO.NUM.DE.SEMANA",
    YearFrac = "YEARFRAC" | "FRAC.AÑO",
    Time = "TIME" | "NSHORA",
    DateValue = "DATEVALUE" | "FECHANUMERO",
    TimeValue = "TIMEVALUE" | "HORANUMERO",
    Days360 = "DAYS360" | "DIAS360",
    // Lookup, reference and dynamic arrays
    Lookup = "LOOKUP" | "BUSCAR",
    Offset = "OFFSET" | "DESREF",
    Indirect = "INDIRECT" | "INDIRECTO",
    Address = "ADDRESS" | "DIRECCION",
    XMatch = "XMATCH" | "COINCIDIRX",
    ChooseCols = "CHOOSECOLS" | "ELEGIRCOLS",
    ChooseRows = "CHOOSEROWS" | "ELEGIRFILAS",
    Take = "TAKE" | "TOMAR",
    Drop = "DROP" | "EXCLUIR",
    Filter = "FILTER" | "FILTRAR",
    Sort = "SORT" | "ORDENAR",
    SortBy = "SORTBY" | "ORDENARPOR",
    Unique = "UNIQUE" | "UNICOS",
    VStack = "VSTACK" | "APILARV",
    HStack = "HSTACK" | "APILARH",
    Transpose = "TRANSPOSE" | "TRANSPONER",
    Expand = "EXPAND" | "EXPANDIR",
    ToCol = "TOCOL" | "ENCOL",
    ToRow = "TOROW" | "ENFILA",
    WrapRows = "WRAPROWS" | "AJUSTARFILAS",
    WrapCols = "WRAPCOLS" | "AJUSTARCOLS",
    Areas = "AREAS",
    FormulaText = "FORMULATEXT" | "FORMULATEXTO",
    Hyperlink = "HYPERLINK" | "HIPERVINCULO",
    // Names and lambdas
    Let = "LET",
    Lambda = "LAMBDA",
    Map = "MAP",
    Reduce = "REDUCE",
    Scan = "SCAN",
    ByRow = "BYROW",
    ByCol = "BYCOL",
    MakeArray = "MAKEARRAY",
    IsOmitted = "ISOMITTED",
    Switch = "SWITCH" | "CAMBIAR",
    // Database
    DAverage = "DAVERAGE" | "BDPROMEDIO",
    DCount = "DCOUNT" | "BDCONTAR",
    DCountA = "DCOUNTA" | "BDCONTARA",
    DGet = "DGET" | "BDEXTRAER",
    DMax = "DMAX" | "BDMAX",
    DMin = "DMIN" | "BDMIN",
    DProduct = "DPRODUCT" | "BDPRODUCTO",
    DStdev = "DSTDEV" | "BDDESVEST",
    DStdevP = "DSTDEVP" | "BDDESVESTP",
    DSum = "DSUM" | "BDSUMA",
    DVar = "DVAR" | "BDVAR",
    DVarP = "DVARP" | "BDVARP",
    // Summaries
    GroupBy = "GROUPBY" | "AGRUPARPOR",
    PivotBy = "PIVOTBY" | "PIVOTARPOR",
    PercentOf = "PERCENTOF",
    TrimRange = "TRIMRANGE" | "RECORTARRANGO" | "RECORTAR.RANGO",
    GetPivotData = "GETPIVOTDATA" | "IMPORTARDATOSDINAMICOS",
    // Financial
    Rate = "RATE" | "TASA",
    Nper = "NPER",
    Irr = "IRR" | "TIR",
    Xirr = "XIRR" | "TIR.NO.PER",
    Xnpv = "XNPV" | "VNA.NO.PER",
    Ipmt = "IPMT" | "PAGOINT",
    Ppmt = "PPMT" | "PAGOPRIN",
    Sln = "SLN",
    Syd = "SYD",
    Db = "DB",
    Ddb = "DDB",
    Vdb = "VDB" | "DVS",
    Mirr = "MIRR" | "TIRM",
    Effect = "EFFECT" | "INT.EFECTIVO",
    Nominal = "NOMINAL" | "TASA.NOMINAL",
    CumIpmt = "CUMIPMT" | "PAGO.INT.ENTRE",
    CumPrinc = "CUMPRINC" | "PAGO.PRINC.ENTRE",
    IsPmt = "ISPMT" | "INT.PAGO.DIR",
    Rri = "RRI",
    PDuration = "PDURATION" | "P.DURACION",
    FvSchedule = "FVSCHEDULE" | "VF.PLAN",
    DollarDe = "DOLLARDE" | "MONEDA.DEC",
    DollarFr = "DOLLARFR" | "MONEDA.FRAC",
    EuroConvert = "EUROCONVERT",
    AccrInt = "ACCRINT" | "INT.ACUM",
    AccrIntM = "ACCRINTM" | "INT.ACUM.V",
    AmorDegrc = "AMORDEGRC" | "AMORTIZ.PROGRE",
    AmorLinc = "AMORLINC" | "AMORTIZ.LIN",
    CoupDayBs = "COUPDAYBS" | "CUPON.DIAS.L1",
    CoupDays = "COUPDAYS" | "CUPON.DIAS",
    CoupDaysNc = "COUPDAYSNC" | "CUPON.DIAS.L2",
    CoupNcd = "COUPNCD" | "CUPON.FECHA.L2",
    CoupNum = "COUPNUM" | "CUPON.NUM",
    CoupPcd = "COUPPCD" | "CUPON.FECHA.L1",
    Disc = "DISC" | "TASA.DESC",
    Duration = "DURATION" | "DURACION",
    IntRate = "INTRATE" | "TASA.INT",
    MDuration = "MDURATION" | "DURACION.MODIF",
    OddFPrice = "ODDFPRICE" | "PRECIO.PER.IRREGULAR.1",
    OddFYield = "ODDFYIELD" | "RENDTO.PER.IRREGULAR.1",
    OddLPrice = "ODDLPRICE" | "PRECIO.PER.IRREGULAR.2",
    OddLYield = "ODDLYIELD" | "RENDTO.PER.IRREGULAR.2",
    Price = "PRICE" | "PRECIO",
    PriceDisc = "PRICEDISC" | "PRECIO.DESCUENTO",
    PriceMat = "PRICEMAT" | "PRECIO.VENCIMIENTO",
    Received = "RECEIVED" | "CANTIDAD.RECIBIDA",
    TBillEq = "TBILLEQ" | "LETRA.DE.TES.EQV.A.BONO",
    TBillPrice = "TBILLPRICE" | "LETRA.DE.TES.PRECIO",
    TBillYield = "TBILLYIELD" | "LETRA.DE.TES.RENDTO",
    Yield = "YIELD" | "RENDTO",
    YieldDisc = "YIELDDISC" | "RENDTO.DESC",
    YieldMat = "YIELDMAT" | "RENDTO.VENCTO",
    // Information
    IsEven = "ISEVEN" | "ES.PAR",
    IsOdd = "ISODD" | "ES.IMPAR",
    Type = "TYPE" | "TIPO",
    ErrorType = "ERROR.TYPE" | "TIPO.DE.ERROR",
    CellInfo = "CELL" | "CELDA",
    IsLogical = "ISLOGICAL" | "ESLOGICO",
    IsNonText = "ISNONTEXT" | "ESNOTEXTO",
    IsRef = "ISREF" | "ESREF",
    IsErr = "ISERR" | "ESERR",
    IsFormula = "ISFORMULA" | "ESFORMULA",
    SheetNum = "SHEET" | "HOJA",
    SheetsNum = "SHEETS" | "HOJAS",
    // Engineering
    Bin2Dec = "BIN2DEC" | "BIN.A.DEC",
    Bin2Hex = "BIN2HEX" | "BIN.A.HEX",
    Bin2Oct = "BIN2OCT" | "BIN.A.OCT",
    Dec2Bin = "DEC2BIN" | "DEC.A.BIN",
    Dec2Hex = "DEC2HEX" | "DEC.A.HEX",
    Dec2Oct = "DEC2OCT" | "DEC.A.OCT",
    Hex2Bin = "HEX2BIN" | "HEX.A.BIN",
    Hex2Dec = "HEX2DEC" | "HEX.A.DEC",
    Hex2Oct = "HEX2OCT" | "HEX.A.OCT",
    Oct2Bin = "OCT2BIN" | "OCT.A.BIN",
    Oct2Dec = "OCT2DEC" | "OCT.A.DEC",
    Oct2Hex = "OCT2HEX" | "OCT.A.HEX",
    Convert = "CONVERT" | "CONVERTIR",
    Delta = "DELTA",
    GeStep = "GESTEP" | "MAYOR.O.IGUAL",
    BitAnd = "BITAND" | "BIT.Y",
    BitOr = "BITOR" | "BIT.O",
    BitXor = "BITXOR" | "BIT.XO",
    BitLShift = "BITLSHIFT" | "BIT.DESPLIZQDA",
    BitRShift = "BITRSHIFT" | "BIT.DESPLDCHA",
    Erf = "ERF" | "FUN.ERROR" | "ERF.PRECISE" | "FUN.ERROR.EXACTO",
    Erfc = "ERFC" | "FUN.ERROR.COMPL" | "ERFC.PRECISE" | "FUN.ERROR.COMPL.EXACTO",
    // Complex numbers and Bessel functions
    Complex = "COMPLEX" | "COMPLEJO",
    ImAbs = "IMABS" | "IM.ABS",
    Imaginary = "IMAGINARY" | "IMAGINARIO",
    ImArgument = "IMARGUMENT" | "IM.ANGULO",
    ImConjugate = "IMCONJUGATE" | "IM.CONJUGADA",
    ImCos = "IMCOS" | "IM.COS",
    ImCosh = "IMCOSH" | "IM.COSH",
    ImCot = "IMCOT" | "IM.COT",
    ImCsc = "IMCSC" | "IM.CSC",
    ImCsch = "IMCSCH" | "IM.CSCH",
    ImDiv = "IMDIV" | "IM.DIV",
    ImExp = "IMEXP" | "IM.EXP",
    ImLn = "IMLN" | "IM.LN",
    ImLog10 = "IMLOG10" | "IM.LOG10",
    ImLog2 = "IMLOG2" | "IM.LOG2",
    ImPower = "IMPOWER" | "IM.POT",
    ImProduct = "IMPRODUCT" | "IM.PRODUCT",
    ImReal = "IMREAL" | "IM.REAL",
    ImSec = "IMSEC" | "IM.SEC",
    ImSech = "IMSECH" | "IM.SECH",
    ImSin = "IMSIN" | "IM.SENO",
    ImSinh = "IMSINH" | "IM.SENOH",
    ImSqrt = "IMSQRT" | "IM.RAIZ2",
    ImSub = "IMSUB" | "IM.SUSTR",
    ImSum = "IMSUM" | "IM.SUM",
    ImTan = "IMTAN" | "IM.TAN",
    BesselI = "BESSELI",
    BesselJ = "BESSELJ",
    BesselK = "BESSELK",
    BesselY = "BESSELY",
    // Math
    EcmaCeiling = "ECMA.CEILING" | "MULTIPLO.SUPERIOR.ECMA",
    // Compatibility: older versions kept under their own names.
    BetaDistLegacy = "BETADIST" | "DISTR.BETA",
    BetaInvLegacy = "BETAINV" | "DISTR.BETA.INV",
    HypgeomDistLegacy = "HYPGEOMDIST" | "DISTR.HIPERGEOM",
    LognormDistLegacy = "LOGNORMDIST" | "DISTR.LOG.NORM",
    // Google Sheets
    Query = "QUERY",
    ArrayFormula = "ARRAYFORMULA",
    Split = "SPLIT",
    Join = "JOIN",
    Flatten = "FLATTEN",
    CountUnique = "COUNTUNIQUE",
    ArrayConstrain = "ARRAY_CONSTRAIN",
    SortN = "SORTN",
    GAdd = "ADD",
    GMinus = "MINUS",
    GMultiply = "MULTIPLY",
    GDivide = "DIVIDE",
    GPow = "POW",
    GEq = "EQ",
    GNe = "NE",
    GGt = "GT",
    GGte = "GTE",
    GLt = "LT",
    GLte = "LTE",
    GUMinus = "UMINUS",
    GUPlus = "UPLUS",
    GUnaryPercent = "UNARY_PERCENT",
    IsEmail = "ISEMAIL",
    IsUrl = "ISURL",
    ToText = "TO_TEXT",
    ToPercent = "TO_PERCENT",
    ToDollars = "TO_DOLLARS",
    ToDate = "TO_DATE",
    ToPureNumber = "TO_PURE_NUMBER",
}

mod array;
mod database;
mod groupby;
pub mod google;
mod gquery;
mod date;
mod eng;
mod complex;
mod finance;
mod securities;
mod math;
mod stats;
pub(crate) use date::text_number;
pub(crate) use stats::norm_inv;
mod text;
mod text2;
mod web;

/// Bits for the arguments a function takes as a whole range or array
/// (the rest are single values, and arrays there repeat the call).
const ALL: u64 = u64::MAX;

impl Func {
    pub fn is_volatile(self) -> bool {
        matches!(self, Func::Rand | Func::RandBetween | Func::Today | Func::Now | Func::RandArray | Func::Offset | Func::Indirect | Func::CellInfo | Func::Info | Func::GetPivotData)
    }

    pub(crate) fn array_params(self) -> u64 {
        use Func::*;
        match self {
            Sum | Average | Min | Max | Count | CountA | CountBlank | Product | Median | Stdev | Var | And | Or | Xor | SumProduct | Concat | Npv | Subtotal | Rows | Columns | Row | Column => ALL,
            TextJoin => !0b11,
            SumIf | AverageIf => 0b101,
            CountIf => 0b1,
            // Ranges at 0, 1, 3, 5...; criteria at 2, 4...
            SumIfs => 0xAAAA_AAAA_AAAA_AAAB,
            CountIfs => 0x5555_5555_5555_5555,
            VLookup | HLookup | Match => 0b10,
            XLookup => 0b110,
            Index | Large | Small => 0b1,
            Rank => 0b10,
            Choose => !0b1,
            If | Ifs | IfError | IfNa | Not | IsBlank | IsNumber | IsText | IsError | IsNa => 0,
            f => math::array_params(f)
                .or_else(|| complex::array_params(f))
                .or_else(|| stats::array_params(f))
                .or_else(|| text::array_params(f))
                .or_else(|| text2::array_params(f))
                .or_else(|| web::array_params(f))
                .or_else(|| date::array_params(f))
                .or_else(|| array::array_params(f))
                .or_else(|| database::array_params(f))
                .or_else(|| groupby::array_params(f))
                .or_else(|| google::array_params(f))
                .or_else(|| finance::array_params(f))
                .or_else(|| securities::array_params(f))
                .or_else(|| eng::array_params(f))
                .unwrap_or(0),
        }
    }

    /// The arguments calculated for whole arrays even in a plain formula
    /// (Excel's array-class parameters); the other `array_params` take
    /// ranges whole, but what is calculated in them is one value.
    pub(crate) fn array_class(self) -> u64 {
        use Func::*;
        match self {
            SumProduct | SumX2My2 | SumX2Py2 | SumXMy2 | Correl | Pearson | Rsq | Covar | CovarianceS | Slope | Intercept | Steyx | Frequency | ChisqTest | FTest => ALL,
            MMult | Linest | Logest | TTest | Prob => 0b11,
            MInverse | MDeterm | Transpose => 0b1,
            Trend | Growth => 0b111,
            Forecast | Lookup => 0b110,
            Aggregate => !0b11,
            ChooseCols | ChooseRows | Take | Drop | Filter | Sort | SortBy | Unique | VStack | HStack | Expand | ToCol | ToRow | WrapRows | WrapCols | Map | Reduce | Scan | ByRow | ByCol | MakeArray | GroupBy | PivotBy | PercentOf => ALL,
            XLookup => 0b110,
            XMatch => 0b10,
            Query | ArrayFormula | Flatten | SortN | Join | CountUnique => ALL,
            ArrayConstrain => 0b1,
            _ => 0,
        }
    }

    /// Whether argument `i` of `n` is a function (LAMBDA or a bare name).
    pub(crate) fn takes_lambda(self, i: usize, n: usize) -> bool {
        use Func::*;
        match self {
            GroupBy | MakeArray => i == 2,
            PivotBy => i == 3,
            ByRow | ByCol => i == 1,
            Map | Reduce | Scan => i + 1 == n,
            _ => false,
        }
    }

    /// Prefix Excel expects in files before functions newer than 2007.
    /// Name in OpenDocument formulas, as LibreOffice writes it.
    pub fn odf_name(self) -> std::borrow::Cow<'static, str> {
        use Func::*;
        use std::borrow::Cow;
        match self {
            FDist => Cow::Borrowed("FDIST"),
            FInv => Cow::Borrowed("FINV"),
            SkewP => Cow::Borrowed("SKEWP"),
            FormulaText => Cow::Borrowed("FORMULA"),
            NormSDistLegacy => Cow::Borrowed("LEGACY.NORMSDIST"),
            NormSInvLegacy => Cow::Borrowed("LEGACY.NORMSINV"),
            TDistLegacy => Cow::Borrowed("LEGACY.TDIST"),
            Dbcs => Cow::Borrowed("JIS"),
            BahtText | EncodeUrl | FilterXml => Cow::Owned(format!("COM.MICROSOFT.{}", self.name())),
            Ifs | Ceiling | Floor | XLookup | Concat | TextJoin | CeilingMath | FloorMath | CeilingPrecise | FloorPrecise | Aggregate | Sequence | RandArray | MaxIfs | MinIfs | ModeMult | PercentileExc | QuartileExc | PercentRankExc | RankAvg | CovarianceS | NormDist | NormInv | NormSDist | NormSInv | LognormDist | LognormInv | TDist | TDist2T | TDistRT | TInv | TInv2T | TTest | ChisqDist | ChisqDistRT | ChisqInv | ChisqInvRT | ChisqTest | FDistRT | FInvRT | FTest | BinomDist | BinomInv | BinomDistRange | PoissonDist | ExponDist | GammaDist | GammaInv | BetaDist | BetaInv | WeibullDist | HypgeomDist | NegbinomDist | ConfidenceNorm | ConfidenceT | ZTest | TextBefore | TextAfter | TextSplit | XMatch | ChooseCols | ChooseRows | Take | Drop | Filter | Sort | SortBy | Unique | VStack | HStack | Expand | ToCol | ToRow | WrapRows | WrapCols | Let | Switch | ForecastEts | ForecastEtsConfint | ForecastEtsSeasonality | ForecastEtsStat => Cow::Owned(format!("COM.MICROSOFT.{}", self.name())),
            // Not known to LibreOffice: kept the way it keeps them.
            RegexTest | RegexExtract | RegexReplace | ValueToText | ArrayToText | NetworkDaysIntl | WorkdayIntl | Lambda | Map | Reduce | Scan | ByRow | ByCol | MakeArray | IsOmitted => Cow::Owned(format!("_xlfn.{}", self.name())),
            EcmaCeiling | GroupBy | PivotBy | PercentOf | TrimRange | Image => Cow::Owned(format!("_xlfn.{}", self.name())),
            _ => Cow::Borrowed(self.name()),
        }
    }

    pub fn xlsx_prefix(self) -> &'static str {
        use Func::*;
        match self {
            Filter | Sort => "_xlfn._xlws.",
            BahtText | EncodeUrl | FilterXml => "_xlfn.",
            Aggregate | Acot | Acoth | Arabic | Base | BetaDist | BetaInv | BitAnd | BitLShift | BitOr | BitRShift | BitXor | CeilingMath | CeilingPrecise | ChisqDist | ChisqDistRT | ChisqInv | ChisqInvRT | ChisqTest | Combina | Concat | ConfidenceNorm | ConfidenceT | Cot | Coth | CovarianceS | Csc | Csch | Days | Decimal | ExponDist | FDist | FDistRT | FInv | FInvRT | FTest | FloorMath | FloorPrecise | FormulaText | Gamma | GammaDist | GammaInv | Gauss | HypgeomDist | IfNa | Ifs | IsFormula | IsoWeekNum | LognormDist | LognormInv | MaxIfs | MinIfs | ModeMult | MUnit | NegbinomDist | NetworkDaysIntl | NormDist | NormInv | NormSDist | NormSInv | NumberValue | PDuration | PercentileExc | PercentRankExc | Permutationa | Phi | PoissonDist | QuartileExc | RankAvg | Rri | Sec | Sech | SheetNum | SheetsNum | SkewP | Switch | TDist | TDist2T | TDistRT | TInv | TInv2T | TTest | TextJoin | UniChar | Unicode | WeibullDist | WorkdayIntl | Xor | ZTest | XLookup | XMatch | Unique | Sequence | RandArray | SortBy | Let | Lambda | TextBefore | TextAfter | TextSplit | VStack | HStack | Take | Drop | ChooseCols | ChooseRows | Expand | ToCol | ToRow | WrapRows | WrapCols | Map | Reduce | Scan | ByRow | ByCol | MakeArray | IsOmitted | ArrayToText | ValueToText | RegexTest | RegexExtract | RegexReplace | BinomDist | BinomInv | BinomDistRange | ForecastEts | ForecastEtsConfint | ForecastEtsSeasonality | ForecastEtsStat => "_xlfn.",
            ImCosh | ImCot | ImCsc | ImCsch | ImSec | ImSech | ImSinh | ImTan | EcmaCeiling | GroupBy | PivotBy | PercentOf | TrimRange | Image => "_xlfn.",
            _ => "",
        }
    }
}

/// Access to cell values from the evaluator.
pub trait EvalContext {
    fn value(&self, sheet: usize, cell: CellRef) -> Value;
    fn sheet_index(&self, name: &str) -> Option<usize>;
    /// Calls `f` for every non-empty cell inside `range`, in row-major order
    /// is NOT guaranteed; functions that need order use [`EvalContext::value`].
    fn for_each(&self, sheet: usize, range: &Range, f: &mut dyn FnMut(CellRef, &Value));
    /// Last used row/column (to clamp whole-column references).
    fn used_extent(&self, sheet: usize) -> CellRef;
    fn row_hidden(&self, _sheet: usize, _row: u32) -> Hidden {
        Hidden::No
    }
    /// Whether the cell holds a SUBTOTAL formula (skipped by SUBTOTAL).
    fn is_subtotal(&self, _sheet: usize, _cell: CellRef) -> bool {
        false
    }
    /// The area a dynamic array formula at `cell` spills into.
    fn spill_range(&self, _sheet: usize, _cell: CellRef) -> Option<Range> {
        None
    }
    /// The value of function `name` of the workbook's macros, called from
    /// `cell` (None: there is no such function).
    /// Whether the workbook's macros have function `name`.
    fn has_udf(&self, _name: &str) -> bool {
        false
    }
    fn call_udf(&self, _name: &str, _args: Vec<crate::udf::UdfArg>, _sheet: usize, _cell: CellRef) -> Option<Value> {
        None
    }
    /// The formula in a cell, as shown in the formula bar.
    fn formula_text(&self, _sheet: usize, _cell: CellRef) -> Option<String> {
        None
    }
    /// The sheet evaluates formulas with Lotus 1-2-3 rules (text as 0).
    fn text_as_zero(&self, _sheet: usize) -> bool {
        false
    }
    fn sheet_count(&self) -> usize {
        1
    }
    fn sheet_name(&self, _sheet: usize) -> Option<String> {
        None
    }
    /// The range a structured reference in `cell` stands for.
    fn table_ref(&self, _sheet: usize, _cell: CellRef, _r: &crate::table::StructRef) -> Result<(usize, Range), ErrorKind> {
        Err(ErrorKind::Ref)
    }
    /// Number format code and column width (in characters), for CELL.
    fn cell_format(&self, _sheet: usize, _cell: CellRef) -> (String, f32) {
        (String::from("General"), 8.43)
    }
    /// What a defined name used on `sheet` stands for.
    /// Saved value of cells in another workbook (`[1]Sheet1`).
    fn external(&self, _sheet: &str, _range: &Range) -> Value {
        Value::Error(ErrorKind::Ref)
    }
    fn defined_name(&self, _sheet: usize, _name: &str) -> Option<Arc<Expr>> {
        None
    }
    /// Whether calculation is manual (for INFO).
    fn manual_calc(&self) -> bool {
        false
    }

    /// GETPIVOTDATA: the value of `field` for the (field, item) pairs in
    /// the pivot table holding `cell`.
    fn pivot_value(&self, _sheet: usize, _cell: CellRef, _field: &str, _pairs: &[(String, Value)]) -> Option<Value> {
        None
    }
}

/// Why a row is hidden, for SUBTOTAL.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hidden {
    No,
    Manual,
    /// Hidden by an autofilter.
    Filtered,
}

pub struct Env<'a> {
    pub ctx: &'a dyn EvalContext,
    pub sheet: usize,
    pub cell: CellRef,
    pub locale: Locale,
    /// Set when a scalar argument turned out to be an array: the call is
    /// then repeated for each element (lifting).
    lift: Cell<bool>,
    /// Names bound by LET and LAMBDA, innermost last.
    scope: RefCell<Vec<(Arc<str>, Bound)>>,
    depth: Cell<u32>,
    /// The text-as-0 setting before this one, put back when done.
    outer_zero: bool,
}

impl Drop for Env<'_> {
    fn drop(&mut self) {
        crate::value::set_text_zero(self.outer_zero);
        EVAL_BASE.with(|b| b.set((b.get().0, b.get().1 - 1)));
    }
}

thread_local! {
    /// Where on the stack the outermost evaluation of this thread began,
    /// and how many are under way.
    static EVAL_BASE: Cell<(usize, u32)> = const { Cell::new((0, 0)) };
    /// What the outermost evaluation under way has spent, shared with
    /// those it starts (database criteria): names expanded and lambdas
    /// called (names using each other twice over would otherwise take
    /// exponential time), steps of wildcard matching left, and when it
    /// began (a lambda called millions of times over large arrays stops
    /// when it has run too long).
    static EVAL_WORK: Cell<(u32, usize, Option<web_time::Instant>)> = const { Cell::new((0, 0, None)) };
}

/// Steps of wildcard matching all of a formula may take.
const WILD_WORK: usize = 1 << 28;

/// Array elements all of a formula may make (an operator over two large
/// arrays repeated thousands of times would otherwise run for hours).
const ARRAY_WORK: usize = 1 << 30;

thread_local! {
    /// Array elements the formula being evaluated may still make.
    static ARRAY_LEFT: Cell<usize> = const { Cell::new(usize::MAX) };
    /// Evaluations sharing one budget (see [`shared_budget`]).
    static SHARED: Cell<bool> = const { Cell::new(false) };
}

/// Takes `n` array elements (or cells read) from the formula's budget:
/// false once spent, or once the formula has run out of time.
pub(crate) fn array_charge(n: usize) -> bool {
    if EVAL_BASE.with(|b| b.get().1) == 0 {
        return true;
    }
    let late = n >= 1024 && EVAL_WORK.with(|w| w.get().2.is_some_and(|b| b.elapsed().as_secs() >= 30));
    ARRAY_LEFT.with(|c| {
        let left = if late { 0 } else { c.get() };
        c.set(left.saturating_sub(n));
        left >= n
    })
}

thread_local! {
    /// Cells read one by one not yet charged to [`array_charge`].
    static TICKS: Cell<usize> = const { Cell::new(0) };
}

/// Whether the formula has run past its time (each repeated call may
/// read a million cells).
fn late() -> bool {
    EVAL_BASE.with(|b| b.get().1) > 0 && EVAL_WORK.with(|w| w.get().2.is_some_and(|b| b.elapsed().as_secs() >= 30))
}

/// Counts one cell read or repeated call toward the formula's array
/// budget, 4096 at a time; false once the budget is spent.
pub(crate) fn tick() -> bool {
    if EVAL_BASE.with(|b| b.get().1) == 0 {
        return true;
    }
    if ARRAY_LEFT.with(Cell::get) == 0 {
        return false;
    }
    TICKS.with(|t| {
        let n = t.get() + 1;
        t.set(n % 4096);
        n < 4096 || array_charge(n)
    })
}

/// [`EvalContext::for_each`], the cells read counted against the
/// formula's budget (lookups over whole columns, again and again).
pub(crate) fn scan<C: EvalContext + ?Sized>(ctx: &C, sheet: usize, r: &Range, f: &mut dyn FnMut(CellRef, &Value)) {
    let mut n = 0usize;
    ctx.for_each(sheet, r, &mut |c, v| {
        n += 1;
        f(c, v)
    });
    array_charge(n.max(1));
}

/// Runs `f` with the formulas it evaluates sharing one budget of `secs`
/// seconds and `elements` array elements, as one formula would (the
/// conditional formats of a screen, each cell its own formula).
pub fn shared_budget<T>(secs: u64, elements: usize, f: impl FnOnce() -> T) -> T {
    let before = SHARED.with(|s| s.replace(true));
    if !before {
        // Born earlier, the formulas' 30 s run out after `secs`.
        let now = web_time::Instant::now();
        let born = now.checked_sub(web_time::Duration::from_secs(30u64.saturating_sub(secs))).unwrap_or(now);
        EVAL_WORK.with(|w| w.set((0, WILD_WORK, Some(born))));
        ARRAY_LEFT.with(|c| c.set(elements));
    }
    struct Back(bool);
    impl Drop for Back {
        fn drop(&mut self) {
            SHARED.with(|s| s.set(self.0));
        }
    }
    let _back = Back(before);
    f()
}

/// Steps one wildcard match may take now: at most `most`, and what the
/// formula being evaluated has left (outside one, `most`).
fn wild_budget(most: usize) -> usize {
    if EVAL_BASE.with(|b| b.get().1) == 0 {
        return most;
    }
    EVAL_WORK.with(|w| w.get().1.min(most))
}

/// Takes `n` steps from the formula's wildcard budget.
fn wild_spent(n: usize) {
    if EVAL_BASE.with(|b| b.get().1) > 0 {
        EVAL_WORK.with(|w| {
            let (a, left, born) = w.get();
            w.set((a, left.saturating_sub(n), born));
        });
    }
}

/// The stack an evaluation may take: names, lambdas and nested formulas
/// multiply each other's depth, and running out of stack aborts the
/// program. Threads have 2 MiB or more (the program's main thread 8 MiB
/// everywhere); unoptimised builds take several times the stack.
const EVAL_STACK: usize = if cfg!(debug_assertions) { 1536 << 10 } else { 1 << 20 };

/// The address of a local of the caller, to measure the stack by.
#[inline(never)]
fn stack_here() -> usize {
    let x = 0u8;
    std::hint::black_box(&x) as *const u8 as usize
}

/// What a LET or LAMBDA name stands for.
#[derive(Clone, Debug)]
pub(crate) enum Bound {
    V(Value),
    /// A reference keeps working where functions need cells (SUMIF...).
    R(usize, Range),
    /// A LAMBDA parameter the caller left out (ISOMITTED).
    Omitted,
}

/// An argument evaluated in reference context.
pub(crate) enum Arg {
    V(Value),
    R(usize, Range),
    A(Arc<Matrix>),
}

/// A two-dimensional argument: cells of a reference or an array.
pub(crate) enum Tab<'e> {
    R(&'e dyn EvalContext, usize, Range),
    A(Arc<Matrix>),
}

impl Tab<'_> {
    pub fn rows(&self) -> usize {
        match self {
            Tab::R(_, _, r) => r.rows() as usize,
            Tab::A(m) => m.rows,
        }
    }

    pub fn cols(&self) -> usize {
        match self {
            Tab::R(_, _, r) => r.cols() as usize,
            Tab::A(m) => m.cols,
        }
    }

    pub fn len(&self) -> usize {
        self.rows() * self.cols()
    }

    pub fn get(&self, r: usize, c: usize) -> Value {
        match self {
            Tab::R(..) if !tick() => Value::Error(ErrorKind::Num),
            Tab::R(ctx, s, rg) => ctx.value(*s, CellRef::new(rg.start.row + r as u32, rg.start.col + c as u32)),
            Tab::A(m) => m.get(r, c).clone(),
        }
    }

    /// Element `i` of a single row or column (row-major otherwise).
    pub fn nth(&self, i: usize) -> Value {
        let w = self.cols();
        self.get(i / w, i % w)
    }

    /// The table as an array; a range too big for one is #NUM!.
    pub fn to_matrix(&self) -> R<Matrix> {
        match self {
            Tab::A(m) => Ok((**m).clone()),
            Tab::R(ctx, s, r) => range_matrix(*ctx, *s, r),
        }
    }
}

pub(crate) type R<T> = Result<T, ErrorKind>;

/// Marks the lambda a function name stands for.
const ETA: &str = "\u{1}eta";

/// A function name used as a value (`GROUPBY(A2:A9,B2:B9,SUM)`): calling
/// it calls the function with the same arguments.
pub(crate) fn eta(f: Func) -> Lambda {
    Lambda { params: Vec::new(), body: Expr::Call { name: f.name().into(), func: Some(f), args: Vec::new() }, captured: vec![(ETA.into(), Value::Empty)] }
}

pub(crate) fn eta_func(l: &Lambda) -> Option<Func> {
    match (&l.body, l.captured.first()) {
        (Expr::Call { func: Some(f), .. }, Some((k, _))) if &**k == ETA => Some(*f),
        _ => None,
    }
}

/// Largest array a formula may build.
pub(crate) const MAX_ARRAY: usize = 4_000_000;

/// The longest text a cell may hold, as in Excel.
pub(crate) const MAX_TEXT: usize = 32767;

/// Bytes past which text being built can no longer fit in a cell.
pub(crate) const MAX_TEXT_BYTES: usize = 4 * MAX_TEXT;

/// Bytes of text an array a formula builds element by element may hold:
/// four million cells of the longest text would not fit in memory.
pub(crate) const MAX_ARRAY_TEXT: usize = 256 << 20;

/// Counts the text of an element of an array being built, false once the
/// array holds more than `MAX_ARRAY_TEXT`.
pub(crate) fn text_fits(v: &Value, total: &mut usize) -> bool {
    if let Value::Text(t) = v {
        *total += t.len();
    }
    *total <= MAX_ARRAY_TEXT
}

/// Whether `t` is longer than a cell may hold.
fn too_long(t: &str) -> bool {
    t.len() > MAX_TEXT && t.chars().count() > MAX_TEXT
}

pub(crate) fn num_ok(n: f64) -> Value {
    if n.is_finite() { Value::Number(n) } else { Value::Error(ErrorKind::Num) }
}

/// The cells of `r` as an array (empty cells stay empty); #NUM! past the
/// largest array.
pub(crate) fn range_matrix(ctx: &dyn EvalContext, sheet: usize, r: &Range) -> R<Matrix> {
    if r.cell_count() as usize > MAX_ARRAY {
        return Err(ErrorKind::Num);
    }
    let (rows, cols) = (r.rows() as usize, r.cols() as usize);
    let mut data = vec![Value::Empty; rows * cols];
    scan(ctx, sheet, r, &mut |c, v| {
        data[(c.row - r.start.row) as usize * cols + (c.col - r.start.col) as usize] = v.clone();
    });
    Ok(Matrix::new(rows, cols, data))
}

/// Applies `f` to a value or to every element of an array.
pub(crate) fn map1(v: Value, f: impl Fn(Value) -> Value) -> Value {
    match v {
        Value::Array(m) if !array_charge(m.data.len()) => Value::Error(ErrorKind::Num),
        Value::Array(m) => Value::Array(Arc::new(Matrix::new(m.rows, m.cols, m.data.iter().cloned().map(f).collect()))),
        v => f(v),
    }
}

/// Applies `f` pairwise, broadcasting single rows, columns and scalars.
pub(crate) fn map2(a: Value, b: Value, f: impl Fn(Value, Value) -> Value) -> Value {
    let dims = |v: &Value| match v {
        Value::Array(m) => (m.rows, m.cols),
        _ => (1, 1),
    };
    let ((ar, ac), (br, bc)) = (dims(&a), dims(&b));
    let (rows, cols) = (ar.max(br), ac.max(bc));
    // A column against a row makes their product.
    if rows.saturating_mul(cols) > MAX_ARRAY || !array_charge(rows * cols) {
        return Value::Error(ErrorKind::Num);
    }
    let at = |v: &Value, r: usize, c: usize| match v {
        Value::Array(m) => m.at(r, c),
        v => v.clone(),
    };
    let mut data = Vec::with_capacity(rows * cols);
    let mut text = 0;
    for r in 0..rows {
        for c in 0..cols {
            let v = f(at(&a, r, c), at(&b, r, c));
            if !text_fits(&v, &mut text) {
                return Value::Error(ErrorKind::Num);
            }
            data.push(v);
        }
    }
    Value::Array(Arc::new(Matrix::new(rows, cols, data)))
}

fn binop(op: BinOp, a: Value, b: Value) -> Value {
    if let Value::Error(k) = a {
        return Value::Error(k);
    }
    if let Value::Error(k) = b {
        return Value::Error(k);
    }
    let arith = |f: fn(f64, f64) -> Value| match (a.to_number(), b.to_number()) {
        (Ok(x), Ok(y)) => f(x, y),
        (Err(k), _) | (_, Err(k)) => Value::Error(k),
    };
    match op {
        BinOp::Add => arith(|x, y| num_ok(x + y)),
        BinOp::Sub => arith(|x, y| num_ok(x - y)),
        BinOp::Mul => arith(|x, y| num_ok(x * y)),
        BinOp::Div => arith(|x, y| if y == 0.0 { Value::Error(ErrorKind::Div0) } else { num_ok(x / y) }),
        BinOp::Pow => arith(|x, y| num_ok(x.powf(y))),
        BinOp::Concat => match (a.to_text(), b.to_text()) {
            (Ok(x), Ok(y)) if x.len() + y.len() > MAX_TEXT && x.chars().count() + y.chars().count() > MAX_TEXT => Value::Error(ErrorKind::Value),
            (Ok(x), Ok(y)) => Value::text(format!("{}{}", x, y)),
            (Err(k), _) | (_, Err(k)) => Value::Error(k),
        },
        _ => {
            // Compare: empty behaves as 0 vs numbers and "" vs text.
            let a = if matches!(a, Value::Empty) && matches!(b, Value::Text(_)) { Value::text("") } else { a };
            let b = if matches!(b, Value::Empty) && matches!(a, Value::Text(_)) { Value::text("") } else { b };
            let ord = a.compare(&b);
            Value::Bool(match op {
                BinOp::Eq => ord == Ordering::Equal,
                BinOp::Ne => ord != Ordering::Equal,
                BinOp::Lt => ord == Ordering::Less,
                BinOp::Le => ord != Ordering::Greater,
                BinOp::Gt => ord == Ordering::Greater,
                _ => ord != Ordering::Less,
            })
        }
    }
}

fn to_arg(v: Value) -> Arg {
    match v {
        Value::Array(m) => Arg::A(m),
        v => Arg::V(v),
    }
}

impl<'a> Env<'a> {
    pub fn new(ctx: &'a dyn EvalContext, sheet: usize, cell: CellRef, locale: Locale) -> Self {
        crate::value::set_coerce_locale(locale);
        let outer_zero = crate::value::set_text_zero(ctx.text_as_zero(sheet));
        EVAL_BASE.with(|b| {
            let (base, n) = b.get();
            b.set((if n == 0 { stack_here() } else { base }, n + 1));
            if n == 0 && !SHARED.with(Cell::get) {
                EVAL_WORK.with(|w| w.set((0, WILD_WORK, Some(web_time::Instant::now()))));
                ARRAY_LEFT.with(|c| c.set(ARRAY_WORK));
            }
        });
        Env { ctx, sheet, cell, locale, lift: Cell::new(false), scope: RefCell::new(Vec::new()), depth: Cell::new(0), outer_zero }
    }

    pub(crate) fn resolve_sheet(&self, s: &Option<Arc<str>>) -> R<usize> {
        match s {
            None => Ok(self.sheet),
            Some(n) => self.ctx.sheet_index(n).ok_or(ErrorKind::Ref),
        }
    }

    /// Whole-row and whole-column references are cut to the used area so
    /// iteration and arrays stay small.
    pub(crate) fn range_of(&self, sheet: usize, a: &CellAddr, b: &CellAddr, kind: RangeKind) -> Range {
        let r = Range::new(CellRef::new(a.row, a.col), CellRef::new(b.row, b.col));
        if kind == RangeKind::Cells {
            return self.clipped(sheet, r);
        }
        let ext = self.ctx.used_extent(sheet);
        Range::new(r.start, CellRef::new(r.end.row.min(ext.row.max(r.start.row)), r.end.col.min(ext.col.max(r.start.col))))
    }

    /// A range of over a million cells (made by OFFSET, INDIRECT or `:`
    /// too) cut to the used area likewise.
    pub(crate) fn clipped(&self, sheet: usize, r: Range) -> Range {
        if r.cell_count() <= 1_000_000 {
            return r;
        }
        let ext = self.ctx.used_extent(sheet);
        Range::new(r.start, CellRef::new(r.end.row.min(ext.row.max(r.start.row)), r.end.col.min(ext.col.max(r.start.col))))
    }

    pub(crate) fn lookup_name(&self, n: &str) -> Option<Bound> {
        if let Some(b) = self.scope.borrow().iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.clone()) {
            return Some(b);
        }
        self.defined_name(n)
    }

    /// A workbook or sheet name: a reference keeps being a reference.
    fn defined_name(&self, n: &str) -> Option<Bound> {
        let expr = self.ctx.defined_name(self.sheet, n)?;
        // Names that use each other in a loop.
        if self.depth.get() > 64 {
            return Some(Bound::V(Value::Error(ErrorKind::Name)));
        }
        if !self.spend() {
            return Some(Bound::V(Value::Error(ErrorKind::Num)));
        }
        let mut e = (*expr).clone();
        // Relative references count from A1 and move with the cell.
        if e.has_relative_refs() {
            e.offset_wrap(self.cell.row as i64, self.cell.col as i64);
        }
        // LET and LAMBDA variables of the caller don't reach into the name.
        let outer = std::mem::take(&mut *self.scope.borrow_mut());
        self.depth.set(self.depth.get() + 1);
        let b = match self.reference(&e) {
            _ if let Some(v) = self.external(&e) => Bound::V(v),
            Some(Ok((s, r))) => Bound::R(s, r),
            Some(Err(k)) => Bound::V(Value::Error(k)),
            None => Bound::V(self.eval(&e)),
        };
        self.depth.set(self.depth.get() - 1);
        *self.scope.borrow_mut() = outer;
        Some(b)
    }

    /// The reference an expression stands for, if it is one.
    pub(crate) fn reference(&self, e: &Expr) -> Option<R<(usize, Range)>> {
        Some(match e {
            Expr::Ref { sheet, addr } => self.resolve_sheet(sheet).map(|s| (s, Range::single(CellRef::new(addr.row, addr.col)))),
            Expr::Range { sheet, a, b, kind } => self.resolve_sheet(sheet).map(|s| (s, self.range_of(s, a, b, *kind))),
            Expr::SpillRef { sheet, addr } => self.resolve_sheet(sheet).and_then(|s| {
                let c = CellRef::new(addr.row, addr.col);
                self.ctx.spill_range(s, c).map(|r| (s, r)).ok_or(ErrorKind::Ref)
            }),
            Expr::Structured(r) => self.ctx.table_ref(self.sheet, self.cell, r).map(|(s, r)| (s, self.clipped(s, r))),
            Expr::Paren(inner) => return self.reference(inner),
            Expr::Binary(BinOp::Isect, l, r) => match (self.reference(l), self.reference(r)) {
                (Some(Ok((s1, a))), Some(Ok((s2, b)))) if s1 == s2 => a.intersect(&b).map(|r| (s1, r)).ok_or(ErrorKind::Null),
                (Some(Err(k)), _) | (_, Some(Err(k))) => Err(k),
                _ => Err(ErrorKind::Value),
            },
            // The smallest range holding both.
            Expr::Binary(BinOp::Range, l, r) => match (self.reference(l), self.reference(r)) {
                (Some(Ok((s1, a))), Some(Ok((s2, b)))) if s1 == s2 => {
                    let start = CellRef::new(a.start.row.min(b.start.row), a.start.col.min(b.start.col));
                    Ok((s1, self.clipped(s1, Range::new(start, CellRef::new(a.end.row.max(b.end.row), a.end.col.max(b.end.col))))))
                }
                (Some(Err(k)), _) | (_, Some(Err(k))) => Err(k),
                _ => Err(ErrorKind::Value),
            },
            Expr::Name(n) => match self.lookup_name(n)? {
                Bound::R(s, r) => Ok((s, r)),
                _ => return None,
            },
            Expr::Call { func: Some(f), args, .. } if matches!(f, Func::Offset | Func::Indirect | Func::Index | Func::TrimRange) => return self.ref_call(*f, args),
            // The reference CHOOSE or IF picks (OFFSET(CHOOSE(n,A,B),1,0)).
            Expr::Call { func: Some(Func::Choose), args, .. } if args.len() >= 2 => {
                let i = match self.eval(&args[0]) {
                    Value::Array(_) => return None,
                    v => v.to_number(),
                };
                return match i {
                    Ok(i) if i >= 1.0 && (i as usize) < args.len() => self.reference(&args[i as usize]),
                    Ok(_) => Some(Err(ErrorKind::Value)),
                    Err(k) => Some(Err(k)),
                };
            }
            Expr::Call { func: Some(Func::If), args, .. } if (2..=3).contains(&args.len()) => {
                let c = match self.eval(&args[0]) {
                    Value::Array(_) => return None,
                    v => v.to_bool(),
                };
                return match c {
                    Ok(true) => self.reference(&args[1]),
                    Ok(false) => self.reference(args.get(2)?),
                    Err(k) => Some(Err(k)),
                };
            }
            _ => return None,
        })
    }

    /// The saved value of a reference into another workbook.
    fn external(&self, e: &Expr) -> Option<Value> {
        match e {
            Expr::Ref { sheet: Some(s), addr } if crate::external::is_external(s) => Some(self.ctx.external(s, &Range::single(CellRef::new(addr.row, addr.col)))),
            Expr::Range { sheet: Some(s), a, b, kind } if crate::external::is_external(s) => {
                use crate::addr::{MAX_COLS, MAX_ROWS};
                let (a, b) = match kind {
                    crate::formula::RangeKind::Cells => (CellRef::new(a.row, a.col), CellRef::new(b.row, b.col)),
                    crate::formula::RangeKind::Cols => (CellRef::new(0, a.col), CellRef::new(MAX_ROWS - 1, b.col)),
                    crate::formula::RangeKind::Rows => (CellRef::new(a.row, 0), CellRef::new(b.row, MAX_COLS - 1)),
                };
                Some(self.ctx.external(s, &Range::new(a, b)))
            }
            _ => None,
        }
    }

    pub(crate) fn arg(&self, e: &Expr) -> Arg {
        if let Some(v) = self.external(e) {
            return to_arg(v);
        }
        match self.reference(e) {
            Some(Ok((s, r))) => Arg::R(s, r),
            Some(Err(k)) => Arg::V(Value::Error(k)),
            None => to_arg(self.eval(e)),
        }
    }

    /// A reference or array argument as a table.
    pub(crate) fn tab(&self, e: &Expr) -> R<Tab<'a>> {
        match self.arg(e) {
            Arg::R(s, r) => Ok(Tab::R(self.ctx, s, r)),
            Arg::A(m) => Ok(Tab::A(m)),
            Arg::V(Value::Error(k)) => Err(k),
            Arg::V(v) => Ok(Tab::A(Arc::new(Matrix::new(1, 1, vec![v])))),
        }
    }

    /// An operand of a binary operator: like `eval`, but an empty cell
    /// stays empty so that it equals "" as well as 0 (`=A1=""`).
    fn operand(&self, e: &Expr) -> Value {
        if let Some(v) = self.external(e) {
            return v;
        }
        match e {
            Expr::Ref { sheet, addr } => match self.resolve_sheet(sheet) {
                Ok(s) => self.ctx.value(s, CellRef::new(addr.row, addr.col)),
                Err(k) => Value::Error(k),
            },
            // An empty cell stays empty through `+`, parentheses, `@` and a
            // name of one cell (`+A1=""`, `@A:A&"x"`).
            Expr::Paren(x) | Expr::Unary(UnOp::Plus, x) => self.operand(x),
            Expr::Unary(UnOp::At, x) if self.external(x).is_none() => match self.implicit_cell(x) {
                Some(Ok((s, c))) => self.ctx.value(s, c),
                Some(Err(k)) => Value::Error(k),
                None => self.eval(e),
            },
            // The cell INDEX, OFFSET or INDIRECT gives, empty or not.
            Expr::Call { func: Some(Func::Index | Func::Offset | Func::Indirect), .. } => match self.reference(e) {
                Some(Ok((s, r))) if r.is_single() => self.ctx.value(s, r.start),
                _ => self.eval(e),
            },
            Expr::Name(n) => match self.lookup_name(n) {
                Some(Bound::R(s, r)) if r.is_single() => self.ctx.value(s, r.start),
                Some(Bound::R(s, r)) => self.ref_value(s, &r),
                Some(Bound::V(v)) => v,
                Some(Bound::Omitted) => Value::Empty,
                None => self.eval(e),
            },
            _ => self.eval(e),
        }
    }

    pub fn eval(&self, e: &Expr) -> Value {
        if EVAL_BASE.with(|b| b.get().0).abs_diff(stack_here()) > EVAL_STACK {
            return Value::Error(ErrorKind::Num);
        }
        if ARRAY_LEFT.with(Cell::get) == 0 {
            return Value::Error(ErrorKind::Num);
        }
        if let Some(v) = self.external(e) {
            return if v == Value::Empty { Value::Number(0.0) } else { v };
        }
        match e {
            Expr::Number(n) => Value::Number(*n),
            Expr::Text(s) => Value::Text(s.clone()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Error(k) => Value::Error(*k),
            Expr::Lit(v) => v.clone(),
            Expr::Name(n) => match self.lookup_name(n) {
                Some(Bound::V(v)) => v,
                Some(Bound::R(s, r)) => self.ref_value(s, &r),
                Some(Bound::Omitted) => Value::Empty,
                None => match Func::lookup(n) {
                    Some(f) if !matches!(f, Func::Let | Func::Lambda | Func::True | Func::False) => Value::Lambda(Arc::new(eta(f))),
                    _ => Value::Error(ErrorKind::Name),
                },
            },
            Expr::Missing => Value::Empty,
            Expr::Ref { sheet, addr } => match self.resolve_sheet(sheet) {
                Ok(s) => match self.ctx.value(s, CellRef::new(addr.row, addr.col)) {
                    Value::Empty => Value::Number(0.0),
                    v => v,
                },
                Err(k) => Value::Error(k),
            },
            // Several areas only make sense as arguments of functions.
            Expr::Binary(BinOp::Union, ..) => Value::Error(ErrorKind::Value),
            Expr::Range { .. } | Expr::SpillRef { .. } | Expr::Structured(_) | Expr::Binary(BinOp::Isect | BinOp::Range, ..) => match self.reference(e) {
                Some(Ok((s, r))) if matches!(e, Expr::Structured(_)) && r.start == r.end => match self.ctx.value(s, r.start) {
                    Value::Empty => Value::Number(0.0),
                    v => v,
                },
                Some(Ok((s, r))) => self.ref_value(s, &r),
                Some(Err(k)) => Value::Error(k),
                None => Value::Error(ErrorKind::Ref),
            },
            Expr::Array(rows) => {
                let cols = rows.first().map_or(0, Vec::len);
                let data = rows.iter().flatten().map(|x| self.eval(x)).collect();
                Matrix::new(rows.len(), cols, data).into_value()
            }
            Expr::Paren(e) => self.eval(e),
            Expr::Unary(UnOp::At, e) => self.implicit(e),
            // Excel ignores a leading `+` (`=+A1` keeps the text of A1).
            Expr::Unary(UnOp::Plus, e) => self.eval(e),
            Expr::Unary(op, e) => {
                let neg = *op == UnOp::Neg;
                map1(self.eval(e), |v| match v.to_number() {
                    Ok(n) => Value::Number(if neg { -n } else { n }),
                    Err(k) => Value::Error(k),
                })
            }
            Expr::Percent(e) => map1(self.eval(e), |v| match v.to_number() {
                Ok(n) => Value::Number(n / 100.0),
                Err(k) => Value::Error(k),
            }),
            Expr::Binary(op, l, r) => {
                let (a, b) = (self.operand(l), self.operand(r));
                if matches!(a, Value::Array(_)) || matches!(b, Value::Array(_)) {
                    map2(a, b, |x, y| binop(*op, x, y))
                } else {
                    binop(*op, a, b)
                }
            }
            Expr::Call { func: Some(f), args, .. } => self.call(*f, args),
            Expr::Call { func: None, name, args } => match self.lookup_name(name) {
                Some(Bound::V(Value::Lambda(l))) => {
                    let vals = args.iter().map(|a| self.bound(a)).collect();
                    self.invoke(&l, vals)
                }
                // A function of the workbook's macros.
                None if self.ctx.has_udf(name) => {
                    use crate::udf::UdfArg;
                    let vals = args
                        .iter()
                        .map(|a| match self.bound(a) {
                            Bound::V(v) => UdfArg::Value(v),
                            Bound::R(sheet, range) => UdfArg::Range { sheet, range },
                            Bound::Omitted => UdfArg::Missing,
                        })
                        .collect();
                    self.ctx.call_udf(name, vals, self.sheet, self.cell).unwrap_or(Value::Error(ErrorKind::Name))
                }
                _ => Value::Error(ErrorKind::Name),
            },
        }
    }

    /// Value of a reference: one cell, or an array for several.
    fn ref_value(&self, s: usize, r: &Range) -> Value {
        if r.is_single() {
            return match self.ctx.value(s, r.start) {
                Value::Empty => Value::Number(0.0),
                v => v,
            };
        }
        match range_matrix(self.ctx, s, r) {
            Ok(m) => Value::Array(Arc::new(m)),
            Err(k) => Value::Error(k),
        }
    }

    /// `@`: the cell of a range in the formula's row or column, or the
    /// first element of an array.
    fn implicit(&self, e: &Expr) -> Value {
        if let Some(r) = self.implicit_cell(e) {
            return match r {
                Ok((s, c)) => match self.ctx.value(s, c) {
                    Value::Empty => Value::Number(0.0),
                    v => v,
                },
                Err(k) => Value::Error(k),
            };
        }
        match self.eval(e) {
            Value::Array(m) => m.data.first().cloned().unwrap_or(Value::Error(ErrorKind::Calc)),
            v => v,
        }
    }

    /// The cell `@` takes from a reference: the one in the formula's row
    /// or column. `None` when `e` is not a reference.
    fn implicit_cell(&self, e: &Expr) -> Option<R<(usize, CellRef)>> {
        let (s, r) = match self.reference(e)? {
            Ok(x) => x,
            Err(k) => return Some(Err(k)),
        };
        Some(if r.is_single() {
            Ok((s, r.start))
        } else if r.cols() == 1 && r.start.row <= self.cell.row && self.cell.row <= r.end.row {
            Ok((s, CellRef::new(self.cell.row, r.start.col)))
        } else if r.rows() == 1 && r.start.col <= self.cell.col && self.cell.col <= r.end.col {
            Ok((s, CellRef::new(r.start.row, self.cell.col)))
        } else {
            Err(ErrorKind::Value)
        })
    }

    /// The result `n` of a formula: when its last operation is a sum or a
    /// difference and `n` is only the rounding left over from numbers that
    /// cancel out, 0, as Excel shows it (`=0.5-0.4-0.1`).
    pub(crate) fn settle_sum(&self, e: &Expr, n: f64) -> Value {
        if let Expr::Binary(BinOp::Add | BinOp::Sub, l, r) = e
            && let (Ok(a), Ok(b)) = (self.scalar(l).to_number(), self.scalar(r).to_number())
            && n.abs() < a.abs().max(b.abs()) * (1.0 / (1u64 << 48) as f64)
        {
            return Value::Number(0.0);
        }
        Value::Number(n)
    }

    /// A single value where an empty cell stays empty (`scalar` gives 0),
    /// for ISBLANK, ISNUMBER...: one-cell references, whatever gives them
    /// (OFFSET, INDEX, a name, `@`).
    pub(crate) fn scalar_raw(&self, e: &Expr) -> Value {
        let cell = match e {
            Expr::Ref { sheet, addr } if self.external(e).is_none() => Some(self.resolve_sheet(sheet).map(|s| (s, CellRef::new(addr.row, addr.col)))),
            Expr::Unary(UnOp::At, inner) if self.external(inner).is_none() => self.implicit_cell(inner),
            Expr::Name(n) => match self.lookup_name(n) {
                Some(Bound::R(s, r)) if r.is_single() => Some(Ok((s, r.start))),
                Some(Bound::R(s, r)) => return self.scalar(&Expr::Lit(self.ref_value(s, &r))),
                Some(Bound::V(v)) => return self.scalar(&Expr::Lit(v)),
                _ => None,
            },
            Expr::Paren(inner) => return self.scalar_raw(inner),
            Expr::Call { func: Some(Func::Offset | Func::Indirect | Func::Index), .. } => match self.reference(e) {
                Some(Ok((s, r))) if r.is_single() => Some(Ok((s, r.start))),
                _ => None,
            },
            _ => None,
        };
        match cell {
            Some(Ok((s, c))) => self.ctx.value(s, c),
            Some(Err(k)) => Value::Error(k),
            None => self.scalar(e),
        }
    }

    /// Evaluates an argument for binding to a name (references stay references).
    pub(crate) fn bound(&self, e: &Expr) -> Bound {
        if matches!(e, Expr::Missing) {
            return Bound::Omitted;
        }
        match self.arg(e) {
            Arg::R(s, r) => Bound::R(s, r),
            Arg::A(m) => Bound::V(Value::Array(m)),
            Arg::V(v) => Bound::V(v),
        }
    }

    /// Counts a name expansion or lambda call, false past the budget of
    /// one cell's formula.
    fn spend(&self) -> bool {
        const MAX_WORK: u32 = 5_000_000;
        const MAX_SECS: u64 = 30;
        EVAL_WORK.with(|c| {
            let (w, wild, born) = c.get();
            let w = w.saturating_add(1);
            // Out of time counts as out of work, for the calls after too.
            let w = if w % 64 == 0 && born.is_some_and(|b| b.elapsed().as_secs() >= MAX_SECS) { u32::MAX } else { w };
            c.set((w, wild, born));
            w <= MAX_WORK
        })
    }

    /// Calls a LAMBDA value with already evaluated arguments.
    pub(crate) fn invoke(&self, l: &Lambda, args: Vec<Bound>) -> Value {
        if let Some(f) = eta_func(l) {
            let params: Vec<Arc<str>> = (0..args.len()).map(|i| Arc::from(format!("\u{1}{i}"))).collect();
            let body = Expr::Call { name: f.name().into(), func: Some(f), args: params.iter().map(|p| Expr::Name(p.clone())).collect() };
            return self.invoke(&Lambda { params, body, captured: Vec::new() }, args);
        }
        if args.len() > l.params.len() {
            return Value::Error(ErrorKind::Value);
        }
        if self.depth.get() > 200 || !self.spend() {
            return Value::Error(ErrorKind::Num);
        }
        let mut scope: Vec<(Arc<str>, Bound)> = l.captured.iter().map(|(k, v)| (k.clone(), Bound::V(v.clone()))).collect();
        let mut args = args.into_iter();
        for p in &l.params {
            scope.push((p.clone(), args.next().unwrap_or(Bound::Omitted)));
        }
        let saved = self.scope.replace(scope);
        self.depth.set(self.depth.get() + 1);
        let prev = self.lift.replace(false);
        let v = self.eval(&l.body);
        self.lift.set(prev);
        self.depth.set(self.depth.get() - 1);
        self.scope.replace(saved);
        v
    }

    /// Calls a lambda argument with plain values.
    pub(crate) fn apply(&self, l: &Lambda, args: &[Value]) -> Value {
        self.invoke(l, args.iter().map(|v| Bound::V(v.clone())).collect())
    }

    /// Evaluates an argument that must be a LAMBDA.
    pub(crate) fn lambda_arg(&self, e: &Expr) -> R<Arc<Lambda>> {
        match self.eval(e) {
            Value::Lambda(l) => Ok(l),
            Value::Error(k) => Err(k),
            _ => Err(ErrorKind::Value),
        }
    }

    fn let_(&self, a: &[Expr]) -> Value {
        if a.len() < 3 || a.len() % 2 == 0 {
            return Value::Error(ErrorKind::Value);
        }
        let depth = self.scope.borrow().len();
        for pair in a[..a.len() - 1].chunks(2) {
            let Expr::Name(n) = &pair[0] else {
                self.scope.borrow_mut().truncate(depth);
                return Value::Error(ErrorKind::Value);
            };
            let b = self.bound(&pair[1]);
            self.scope.borrow_mut().push((n.clone(), b));
        }
        let v = self.eval(&a[a.len() - 1]);
        self.scope.borrow_mut().truncate(depth);
        v
    }

    fn lambda(&self, a: &[Expr]) -> Value {
        let Some((body, params)) = a.split_last() else { return Value::Error(ErrorKind::Value) };
        let mut names = Vec::with_capacity(params.len());
        for p in params {
            match p {
                Expr::Name(n) if !names.iter().any(|x: &Arc<str>| x.eq_ignore_ascii_case(n)) => names.push(n.clone()),
                _ => return Value::Error(ErrorKind::Value),
            }
        }
        let captured = self
            .scope
            .borrow()
            .iter()
            .map(|(k, b)| {
                let v = match b {
                    Bound::V(v) => v.clone(),
                    Bound::R(s, r) => self.ref_value(*s, r),
                    Bound::Omitted => Value::Empty,
                };
                (k.clone(), v)
            })
            .collect();
        Value::Lambda(Arc::new(Lambda { params: names, body: body.clone(), captured }))
    }

    fn call(&self, f: Func, a: &[Expr]) -> Value {
        match f {
            Func::Let => return self.let_(a),
            Func::Lambda => return self.lambda(a),
            _ => {}
        }
        if let Some(v) = self.multi_area_call(f, a) {
            return v;
        }
        if matches!(f, Func::Match) && let Some(v) = self.match_many(a) {
            return v;
        }
        let prev = self.lift.replace(false);
        let r = self.call_inner(f, a);
        if self.lift.replace(prev) {
            if let Some(v) = self.lift_call(f, a) {
                return v;
            }
        }
        match r {
            Ok(Value::Text(t)) if too_long(&t) => Value::Error(ErrorKind::Value),
            r => r.unwrap_or_else(Value::Error),
        }
    }

    /// The areas an argument stands for when it is a union (`(A1:B2,D4)`)
    /// or a 3D reference (`Sheet1:Sheet3!A1`), one plain reference each.
    fn areas(&self, e: &Expr, out: &mut Vec<Expr>) -> R<bool> {
        match e {
            Expr::Paren(inner) => self.areas(inner, out),
            Expr::Binary(BinOp::Union, l, r) => {
                for side in [l, r] {
                    if !self.areas(side, out)? {
                        out.push((**side).clone());
                    }
                }
                Ok(true)
            }
            Expr::Ref { sheet: Some(s), .. } | Expr::Range { sheet: Some(s), .. } if crate::formula::split_3d(s).is_some() => {
                let (first, last) = crate::formula::split_3d(s).unwrap();
                let (a, b) = (self.ctx.sheet_index(first).ok_or(ErrorKind::Ref)?, self.ctx.sheet_index(last).ok_or(ErrorKind::Ref)?);
                for i in a.min(b)..=a.max(b) {
                    let name: Option<Arc<str>> = Some(self.ctx.sheet_name(i).ok_or(ErrorKind::Ref)?.into());
                    let mut one = e.clone();
                    if let Expr::Ref { sheet, .. } | Expr::Range { sheet, .. } = &mut one {
                        *sheet = name;
                    }
                    out.push(one);
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Calls with a union or 3D argument: functions that take any number of
    /// ranges get one argument per area; AREAS counts them and INDEX picks
    /// one. `None` when no argument has several areas.
    fn multi_area_call(&self, f: Func, a: &[Expr]) -> Option<Value> {
        use Func::*;
        if !a.iter().any(|e| matches!(e, Expr::Paren(_) | Expr::Binary(BinOp::Union, ..) | Expr::Ref { sheet: Some(_), .. } | Expr::Range { sheet: Some(_), .. })) {
            return None;
        }
        let mut parts = Vec::new();
        let mut split = Vec::with_capacity(a.len());
        for e in a {
            let start = parts.len();
            match self.areas(e, &mut parts) {
                Ok(true) => split.push(Some(start..parts.len())),
                Ok(false) => split.push(None),
                Err(k) => return Some(Value::Error(k)),
            }
        }
        if split.iter().all(Option::is_none) {
            return None;
        }
        let variadic = matches!(
            f,
            Sum | Average | AverageA | Min | MinA | Max | MaxA | Count | CountA | Product | Median | Mode | Stdev | StdevP | StdevA | StdevPA | Var | VarP | VarA | VarPA
                | SumSq | AveDev | DevSq | GeoMean | HarMean | Kurt | Skew | And | Or | Xor | Concat
        );
        Some(match f {
            Areas if a.len() == 1 => Value::Number(split[0].as_ref().map_or(1, |r| r.len()) as f64),
            Index if split[0].is_some() && split[1..].iter().all(Option::is_none) => {
                let r = split[0].clone().unwrap();
                let n = match a.get(3) {
                    Some(e) => match self.num(e) {
                        Ok(n) => n as usize,
                        Err(k) => return Some(Value::Error(k)),
                    },
                    None => 1,
                };
                let Some(area) = parts[r].get(n.wrapping_sub(1)).cloned() else { return Some(Value::Error(ErrorKind::Ref)) };
                let mut args = vec![area];
                args.extend(a[1..a.len().min(3)].iter().cloned());
                self.call(Index, &args)
            }
            _ if variadic => {
                let mut args = Vec::with_capacity(parts.len() + a.len());
                for (e, r) in a.iter().zip(&split) {
                    match r {
                        Some(r) => args.extend(parts[r.clone()].iter().cloned()),
                        None => args.push(e.clone()),
                    }
                }
                self.call(f, &args)
            }
            // Other functions see the cells of all the areas as one column
            // (`RANK(A1,(B1,B2,B3))`).
            _ => {
                let mut args = Vec::with_capacity(a.len());
                for (e, r) in a.iter().zip(&split) {
                    let Some(r) = r else {
                        args.push(e.clone());
                        continue;
                    };
                    let mut data = Vec::new();
                    for part in &parts[r.clone()] {
                        match self.arg(part) {
                            Arg::R(s, r) => match range_matrix(self.ctx, s, &r) {
                                Ok(m) => data.extend(m.data),
                                Err(k) => return Some(Value::Error(k)),
                            },
                            Arg::A(m) => data.extend(m.data.iter().cloned()),
                            Arg::V(v) => data.push(v),
                        }
                        if data.len() > MAX_ARRAY {
                            return Some(Value::Error(ErrorKind::Num));
                        }
                    }
                    if data.is_empty() {
                        data.push(Value::Empty);
                    }
                    args.push(Expr::Lit(Matrix::new(data.len(), 1, data).into_value()));
                }
                self.call(f, &args)
            }
        })
    }

    /// Repeats a call for each element of its array arguments that sit
    /// where a single value is expected.
    fn lift_call(&self, f: Func, a: &[Expr]) -> Option<Value> {
        let mask = f.array_params();
        let mut vals: Vec<Option<Value>> = Vec::with_capacity(a.len());
        let (mut rows, mut cols, mut any) = (1, 1, false);
        for (i, e) in a.iter().enumerate() {
            if (i < 64 && mask & (1 << i) != 0) || (i >= 64 && mask == u64::MAX) || matches!(e, Expr::Missing | Expr::Lit(_)) {
                vals.push(None);
                continue;
            }
            let v = self.eval(e);
            if let Value::Array(m) = &v {
                rows = rows.max(m.rows);
                cols = cols.max(m.cols);
                any = true;
            }
            vals.push(Some(v));
        }
        if !any {
            return None;
        }
        if rows * cols > MAX_ARRAY {
            return Some(Value::Error(ErrorKind::Num));
        }
        let mut args: Vec<Expr> = a.to_vec();
        let mut data = Vec::with_capacity(rows * cols);
        let mut text = 0;
        for r in 0..rows {
            for c in 0..cols {
                for (i, v) in vals.iter().enumerate() {
                    if let Some(v) = v {
                        args[i] = Expr::Lit(match v {
                            Value::Array(m) => m.at(r, c),
                            v => v.clone(),
                        });
                    }
                }
                if !tick() || late() {
                    return Some(Value::Error(ErrorKind::Num));
                }
                let prev = self.lift.replace(false);
                let v = self.call_inner(f, &args).unwrap_or_else(Value::Error);
                self.lift.set(prev);
                if !text_fits(&v, &mut text) {
                    return Some(Value::Error(ErrorKind::Num));
                }
                data.push(v);
            }
        }
        Some(Matrix::new(rows, cols, data).into_value())
    }

    /// Evaluates an argument where one value is expected. Arrays make the
    /// enclosing call repeat per element.
    pub(crate) fn scalar(&self, e: &Expr) -> Value {
        let v = self.eval(e);
        if matches!(v, Value::Array(_) | Value::Lambda(_)) {
            self.lift.set(true);
            return Value::Error(ErrorKind::Value);
        }
        v
    }

    /// OFFSET, INDIRECT and INDEX used as references. `None` when the
    /// call doesn't produce one (INDEX over an array).
    pub(crate) fn ref_call(&self, f: Func, a: &[Expr]) -> Option<R<(usize, Range)>> {
        let num = |i: usize, d: f64| -> R<f64> {
            match a.get(i) {
                None | Some(Expr::Missing) => Ok(d),
                Some(e) => self.eval(e).to_number(),
            }
        };
        let rect = |s: usize, r0: i64, c0: i64, h: i64, w: i64| -> R<(usize, Range)> {
            if h < 1 || w < 1 || r0 < 0 || c0 < 0 || r0 + h > MAX_ROWS as i64 || c0 + w > MAX_COLS as i64 {
                return Err(ErrorKind::Ref);
            }
            Ok((s, self.clipped(s, Range::new(CellRef::new(r0 as u32, c0 as u32), CellRef::new((r0 + h - 1) as u32, (c0 + w - 1) as u32)))))
        };
        match f {
            Func::Offset => Some((|| {
                if a.len() < 3 || a.len() > 5 {
                    return Err(ErrorKind::Value);
                }
                let (s, base) = self.reference(&a[0]).ok_or(ErrorKind::Value)??;
                let (dr, dc) = (num(1, 0.0)?.trunc() as i64, num(2, 0.0)?.trunc() as i64);
                let (h, w) = (num(3, base.rows() as f64)?.trunc() as i64, num(4, base.cols() as f64)?.trunc() as i64);
                rect(s, base.start.row as i64 + dr, base.start.col as i64 + dc, h, w)
            })()),
            Func::Indirect => Some((|| {
                if a.is_empty() || a.len() > 2 {
                    return Err(ErrorKind::Value);
                }
                let text = self.eval(&a[0]).to_text()?;
                let a1 = match a.get(1) {
                    None | Some(Expr::Missing) => true,
                    Some(e) => self.eval(e).to_bool()?,
                };
                let e = if a1 { crate::formula::parse_canonical(text.trim()).map_err(|_| ErrorKind::Ref)? } else { parse_r1c1(text.trim(), self.cell).ok_or(ErrorKind::Ref)? };
                match e {
                    Expr::Ref { .. } | Expr::Range { .. } | Expr::Structured(_) => self.reference(&e).unwrap_or(Err(ErrorKind::Ref)),
                    _ => Err(ErrorKind::Ref),
                }
            })()),
            Func::Index => {
                if a.len() < 2 || a.len() > 4 || a[1..].iter().any(|e| matches!(self.eval(e), Value::Array(_))) {
                    return None;
                }
                let (s, r) = match self.reference(&a[0])? {
                    Ok(x) => x,
                    Err(k) => return Some(Err(k)),
                };
                Some((|| {
                    let mut row = num(1, 0.0)?.trunc() as i64;
                    let mut col = num(2, 0.0)?.trunc() as i64;
                    if a.len() == 2 && r.rows() == 1 && r.cols() > 1 {
                        col = row;
                        row = 1;
                    }
                    if row < 0 || col < 0 || row > r.rows() as i64 || col > r.cols() as i64 {
                        return Err(ErrorKind::Ref);
                    }
                    let (r0, h) = if row == 0 { (r.start.row as i64, r.rows() as i64) } else { (r.start.row as i64 + row - 1, 1) };
                    let (c0, w) = if col == 0 && r.cols() > 1 { (r.start.col as i64, r.cols() as i64) } else { (r.start.col as i64 + col.max(1) - 1, 1) };
                    rect(s, r0, c0, h, w)
                })())
            }
            Func::TrimRange => groupby::trim_ref(self, a),
            _ => None,
        }
    }

    // --- argument helpers -------------------------------------------------

    pub(crate) fn num(&self, e: &Expr) -> R<f64> {
        self.scalar(e).to_number()
    }

    pub(crate) fn opt_num(&self, args: &[Expr], i: usize, default: f64) -> R<f64> {
        match args.get(i) {
            None | Some(Expr::Missing) => Ok(default),
            Some(e) => self.num(e),
        }
    }

    /// An argument as text: an empty cell is "" (not "0").
    pub(crate) fn text(&self, e: &Expr) -> R<Arc<str>> {
        self.scalar_raw(e).to_text()
    }

    pub(crate) fn boolean(&self, e: &Expr) -> R<bool> {
        self.scalar(e).to_bool()
    }

    pub(crate) fn opt_bool(&self, args: &[Expr], i: usize, default: bool) -> R<bool> {
        match args.get(i) {
            None | Some(Expr::Missing) => Ok(default),
            Some(e) => self.boolean(e),
        }
    }

    /// Calls `f` for every value of an argument: the cells of a reference
    /// (non-empty only), the elements of an array, or the value itself.
    /// `direct` tells a value typed straight into the call apart.
    pub(crate) fn each(&self, e: &Expr, f: &mut dyn FnMut(&Value, bool)) {
        match self.arg(e) {
            Arg::V(v) => f(&v, true),
            Arg::R(s, r) => scan(self.ctx, s, &r, &mut |_, v| f(v, false)),
            Arg::A(m) => m.data.iter().for_each(|v| f(v, false)),
        }
    }

    /// Numbers from all args. Direct scalars are coerced; range and array
    /// elements only count when numeric (text and booleans are skipped).
    pub(crate) fn numbers(&self, args: &[Expr], out: &mut Vec<f64>) -> R<()> {
        self.fold_numbers(args, (), |_, x| out.push(x)).map(|_| ())
    }

    pub(crate) fn fold_numbers<T>(&self, args: &[Expr], init: T, mut f: impl FnMut(T, f64) -> T) -> R<T> {
        let mut acc = Some(init);
        for a in args {
            let mut err = None;
            self.each(a, &mut |v, direct| {
                let x = match v {
                    Value::Number(n) => *n,
                    Value::Empty => return,
                    Value::Error(k) => {
                        err.get_or_insert(*k);
                        return;
                    }
                    v if direct => match v.to_number() {
                        Ok(n) => n,
                        Err(k) => {
                            err.get_or_insert(k);
                            return;
                        }
                    },
                    _ => return,
                };
                acc = Some(f(acc.take().unwrap(), x));
            });
            if let Some(k) = err {
                return Err(k);
            }
        }
        Ok(acc.unwrap())
    }

    /// A reference argument (functions such as SUMIF need cells).
    pub(crate) fn range_arg(&self, e: &Expr) -> R<(usize, Range)> {
        match self.arg(e) {
            Arg::R(s, r) => Ok((s, r)),
            Arg::V(Value::Error(k)) => Err(k),
            Arg::V(_) | Arg::A(_) => Err(ErrorKind::Value),
        }
    }

    fn call_inner(&self, f: Func, a: &[Expr]) -> R<Value> {
        use Func::*;
        let n = a.len();
        let need = |min: usize, max: usize| if n < min || n > max { Err(ErrorKind::Value) } else { Ok(()) };
        Ok(match f {
            Sum => num_ok(self.fold_numbers(a, 0.0, |x, y| x + y)?),
            Product => num_ok(self.fold_numbers(a, 1.0, |x, y| x * y)?),
            Average => {
                let mut v = Vec::new();
                self.numbers(a, &mut v)?;
                if v.is_empty() {
                    return Err(ErrorKind::Div0);
                }
                num_ok(v.iter().sum::<f64>() / v.len() as f64)
            }
            Min | Max => {
                let mut v = Vec::new();
                self.numbers(a, &mut v)?;
                let r = if f == Min { v.iter().cloned().fold(f64::INFINITY, f64::min) } else { v.iter().cloned().fold(f64::NEG_INFINITY, f64::max) };
                Value::Number(if v.is_empty() { 0.0 } else { r })
            }
            Median => {
                let mut v = Vec::new();
                self.numbers(a, &mut v)?;
                if v.is_empty() {
                    return Err(ErrorKind::Num);
                }
                v.sort_by(|x, y| x.partial_cmp(y).unwrap_or(Ordering::Equal));
                let m = v.len() / 2;
                Value::Number(if v.len() % 2 == 0 { (v[m - 1] + v[m]) / 2.0 } else { v[m] })
            }
            Stdev | Var => {
                let mut v = Vec::new();
                self.numbers(a, &mut v)?;
                if v.len() < 2 {
                    return Err(ErrorKind::Div0);
                }
                let mean = v.iter().sum::<f64>() / v.len() as f64;
                let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (v.len() - 1) as f64;
                Value::Number(if f == Var { var } else { var.sqrt() })
            }
            Large | Small => {
                need(2, 2)?;
                let mut v = Vec::new();
                self.numbers(&a[..1], &mut v)?;
                let k = self.num(&a[1])?.ceil() as usize;
                if k == 0 || k > v.len() {
                    return Err(ErrorKind::Num);
                }
                v.sort_by(|x, y| x.partial_cmp(y).unwrap_or(Ordering::Equal));
                Value::Number(if f == Small { v[k - 1] } else { v[v.len() - k] })
            }
            Subtotal => {
                if n < 2 {
                    return Err(ErrorKind::Value);
                }
                let code = self.num(&a[0])? as i64;
                let (kind, skip_manual) = match code {
                    1..=11 => (code, false),
                    101..=111 => (code - 100, true),
                    _ => return Err(ErrorKind::Value),
                };
                let mut nums = Vec::new();
                let mut filled = 0usize;
                for e in &a[1..] {
                    let (s, r) = self.range_arg(e)?;
                    let mut err = None;
                    scan(self.ctx, s, &r, &mut |c, v| {
                        if self.ctx.is_subtotal(s, c) {
                            return;
                        }
                        match self.ctx.row_hidden(s, c.row) {
                            Hidden::Filtered => return,
                            Hidden::Manual if skip_manual => return,
                            _ => {}
                        }
                        filled += 1;
                        match v {
                            Value::Number(x) => nums.push(*x),
                            Value::Error(k) if err.is_none() => err = Some(*k),
                            _ => {}
                        }
                    });
                    if let Some(k) = err {
                        return Err(k);
                    }
                }
                let len = nums.len() as f64;
                let mean = nums.iter().sum::<f64>() / len;
                let ss = nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>();
                match kind {
                    1 if nums.is_empty() => return Err(ErrorKind::Div0),
                    1 => num_ok(mean),
                    2 => Value::Number(len),
                    3 => Value::Number(filled as f64),
                    4 => Value::Number(if nums.is_empty() { 0.0 } else { nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max) }),
                    5 => Value::Number(if nums.is_empty() { 0.0 } else { nums.iter().cloned().fold(f64::INFINITY, f64::min) }),
                    6 => num_ok(nums.iter().product()),
                    9 => num_ok(nums.iter().sum()),
                    7 | 10 if nums.len() < 2 => return Err(ErrorKind::Div0),
                    8 | 11 if nums.is_empty() => return Err(ErrorKind::Div0),
                    7 => num_ok((ss / (len - 1.0)).sqrt()),
                    8 => num_ok((ss / len).sqrt()),
                    10 => num_ok(ss / (len - 1.0)),
                    _ => num_ok(ss / len),
                }
            }
            Rank => {
                need(2, 3)?;
                let x = self.num(&a[0])?;
                let mut v = Vec::new();
                self.numbers(&a[1..2], &mut v)?;
                let asc = self.opt_num(a, 2, 0.0)? != 0.0;
                if !v.contains(&x) {
                    return Err(ErrorKind::Na);
                }
                let better = v.iter().filter(|&&y| if asc { y < x } else { y > x }).count();
                Value::Number((better + 1) as f64)
            }
            Count => {
                let mut c = 0usize;
                for e in a {
                    self.each(e, &mut |v, direct| {
                        let yes = match v {
                            Value::Number(_) => true,
                            Value::Empty | Value::Error(_) => false,
                            v => direct && v.to_number().is_ok(),
                        };
                        c += yes as usize;
                    });
                }
                Value::Number(c as f64)
            }
            CountA => {
                let mut c = 0usize;
                for e in a {
                    if !matches!(e, Expr::Missing) {
                        self.each(e, &mut |v, _| c += !v.is_empty() as usize);
                    }
                }
                Value::Number(c as f64)
            }
            CountBlank => {
                need(1, 1)?;
                let t = self.tab(&a[0])?;
                let mut filled = 0usize;
                self.each(&a[0], &mut |v, _| {
                    if !v.is_empty() && !matches!(v, Value::Text(t) if t.is_empty()) {
                        filled += 1
                    }
                });
                Value::Number((t.len() - filled) as f64)
            }
            If => {
                need(1, 3)?;
                if self.boolean(&a[0])? {
                    a.get(1).map(|e| self.eval(e)).unwrap_or(Value::Bool(true))
                } else {
                    match a.get(2) {
                        Some(e) => self.eval(e),
                        None => Value::Bool(false),
                    }
                }
            }
            Ifs => {
                if n % 2 != 0 || n == 0 {
                    return Err(ErrorKind::Value);
                }
                for pair in a.chunks(2) {
                    if self.boolean(&pair[0])? {
                        return Ok(self.eval(&pair[1]));
                    }
                }
                return Err(ErrorKind::Na);
            }
            IfError => {
                need(2, 2)?;
                match self.scalar(&a[0]) {
                    Value::Error(_) if !self.lift.get() => self.eval(&a[1]),
                    v => v,
                }
            }
            IfNa => {
                need(2, 2)?;
                match self.scalar(&a[0]) {
                    Value::Error(ErrorKind::Na) => self.eval(&a[1]),
                    v => v,
                }
            }
            And | Or | Xor => {
                let mut vals = Vec::new();
                for e in a {
                    // Written but empty (OR(A1,)) is FALSE, as in Excel.
                    if matches!(e, Expr::Missing) {
                        vals.push(false);
                        continue;
                    }
                    let mut err = None;
                    self.each(e, &mut |v, direct| match v {
                        Value::Empty => {}
                        Value::Number(x) => vals.push(*x != 0.0),
                        Value::Bool(b) => vals.push(*b),
                        Value::Error(k) => err = Some(*k),
                        v if direct => match v.to_bool() {
                            Ok(b) => vals.push(b),
                            Err(k) => err = Some(k),
                        },
                        _ => {}
                    });
                    if let Some(k) = err {
                        return Err(k);
                    }
                }
                if vals.is_empty() {
                    return Err(ErrorKind::Value);
                }
                Value::Bool(match f {
                    And => vals.iter().all(|&b| b),
                    Or => vals.iter().any(|&b| b),
                    _ => vals.iter().filter(|&&b| b).count() % 2 == 1,
                })
            }
            Not => {
                need(1, 1)?;
                Value::Bool(!self.boolean(&a[0])?)
            }
            True => Value::Bool(true),
            False => Value::Bool(false),
            Abs => num_ok(self.num1(a)?.abs()),
            Int => num_ok(self.num1(a)?.floor()),
            Sqrt => {
                let x = self.num1(a)?;
                if x < 0.0 {
                    return Err(ErrorKind::Num);
                }
                num_ok(x.sqrt())
            }
            Exp => num_ok(self.num1(a)?.exp()),
            Ln => {
                let x = self.num1(a)?;
                if x <= 0.0 {
                    return Err(ErrorKind::Num);
                }
                num_ok(x.ln())
            }
            Log10 => {
                let x = self.num1(a)?;
                if x <= 0.0 {
                    return Err(ErrorKind::Num);
                }
                num_ok(x.log10())
            }
            Log => {
                need(1, 2)?;
                let x = self.num(&a[0])?;
                let b = self.opt_num(a, 1, 10.0)?;
                if x <= 0.0 || b <= 0.0 || b == 1.0 {
                    return Err(ErrorKind::Num);
                }
                num_ok(x.log(b))
            }
            Sign => {
                let x = self.num1(a)?;
                Value::Number(if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 })
            }
            Pi => Value::Number(std::f64::consts::PI),
            Round | RoundUp | RoundDown | Trunc => {
                need(1, 2)?;
                let x = self.num(&a[0])?;
                let d = self.opt_num(a, 1, 0.0)?.trunc() as i32;
                let p = 10f64.powi(d);
                // Nudge to counter binary representation (2.675 -> 2.68).
                let y = x * p;
                let y = match f {
                    Round => {
                        let t = y.trunc();
                        if (y - t).abs() >= 0.5 - 1e-9 { t + y.signum() } else { t }
                    }
                    RoundUp => {
                        let t = y.trunc();
                        if (y - t).abs() > 1e-9 { t + y.signum() } else { t }
                    }
                    _ => y.trunc(),
                };
                num_ok(y / p)
            }
            Mod => {
                need(2, 2)?;
                let (x, d) = (self.num(&a[0])?, self.num(&a[1])?);
                if d == 0.0 {
                    return Err(ErrorKind::Div0);
                }
                num_ok(x - d * (x / d).floor())
            }
            Power => {
                need(2, 2)?;
                num_ok(self.num(&a[0])?.powf(self.num(&a[1])?))
            }
            Ceiling | Floor | EcmaCeiling => {
                need(1, 2)?;
                let x = self.num(&a[0])?;
                let s = self.opt_num(a, 1, 1.0)?;
                if s == 0.0 {
                    return Ok(Value::Number(0.0));
                }
                num_ok(if f != Floor { (x / s).ceil() * s } else { (x / s).floor() * s })
            }
            Rand => Value::Number(rand01()),
            RandBetween => {
                need(2, 2)?;
                let (lo, hi) = (self.num(&a[0])?.ceil(), self.num(&a[1])?.floor());
                if hi < lo {
                    return Err(ErrorKind::Num);
                }
                Value::Number((lo + (rand01() * (hi - lo + 1.0)).floor()).min(hi))
            }
            SumIf | AverageIf | CountIf => {
                let (min, max) = if f == CountIf { (2, 2) } else { (2, 3) };
                need(min, max)?;
                let (s, r) = self.range_arg(&a[0])?;
                let crit = Criteria::new(self.scalar(&a[1]), &self.locale);
                let target = if f != CountIf && n == 3 { Some(self.range_arg(&a[2])?) } else { None };
                let (mut sum, mut count) = (0.0, 0usize);
                let mut visit = |c: CellRef, v: &Value| {
                    if crit.matches(v) {
                        count += 1;
                        if f != CountIf {
                            let read;
                            let tv = match target {
                                Some((ts, tr)) => {
                                    read = self.ctx.value(ts, CellRef::new(tr.start.row + c.row - r.start.row, tr.start.col + c.col - r.start.col));
                                    &read
                                }
                                None => v,
                            };
                            if let Value::Number(x) = tv {
                                sum += x;
                            } else if f == AverageIf {
                                count -= 1;
                            }
                        }
                    }
                };
                if r.start.col == r.end.col && !crit.matches(&Value::Empty) {
                    // Only the cells there are, in order: a whole column is
                    // a million rows.
                    scan(self.ctx, s, &r, &mut visit);
                } else {
                    // COUNTIF on a blank criterion must see empty cells, so iterate densely.
                    for row in r.start.row..=r.end.row {
                        for col in r.start.col..=r.end.col {
                            if !tick() {
                                return Err(ErrorKind::Num);
                            }
                            let c = CellRef::new(row, col);
                            visit(c, &self.ctx.value(s, c));
                        }
                    }
                }
                match f {
                    CountIf => Value::Number(count as f64),
                    SumIf => Value::Number(sum),
                    _ => {
                        if count == 0 {
                            return Err(ErrorKind::Div0);
                        }
                        Value::Number(sum / count as f64)
                    }
                }
            }
            SumIfs | CountIfs => {
                let (first, pairs) = if f == SumIfs { (1, &a[1..]) } else { (0, a) };
                if pairs.is_empty() || pairs.len() % 2 != 0 || n < first + 2 {
                    return Err(ErrorKind::Value);
                }
                let conds: Vec<((usize, Range), Criteria)> = pairs
                    .chunks(2)
                    .map(|p| Ok((self.range_arg(&p[0])?, Criteria::new(self.scalar(&p[1]), &self.locale))))
                    .collect::<R<_>>()?;
                let base = if f == SumIfs { self.range_arg(&a[0])? } else { conds[0].0 };
                let (rows, cols) = (base.1.rows(), base.1.cols());
                let (mut sum, mut count) = (0.0, 0usize);
                for dr in 0..rows {
                    for dc in 0..cols {
                        if !tick() {
                            return Err(ErrorKind::Num);
                        }
                        let ok = conds.iter().all(|((s, r), c)| c.matches(&self.ctx.value(*s, CellRef::new(r.start.row + dr, r.start.col + dc))));
                        if ok {
                            count += 1;
                            if let Value::Number(x) = self.ctx.value(base.0, CellRef::new(base.1.start.row + dr, base.1.start.col + dc)) {
                                sum += x;
                            }
                        }
                    }
                }
                Value::Number(if f == SumIfs { sum } else { count as f64 })
            }
            SumProduct => {
                let grids = a.iter().map(|e| self.tab(e).and_then(|t| t.to_matrix())).collect::<R<Vec<_>>>()?;
                let Some(first) = grids.first() else { return Err(ErrorKind::Value) };
                if grids.iter().any(|g| (g.rows, g.cols) != (first.rows, first.cols)) {
                    return Err(ErrorKind::Value);
                }
                let mut total = 0.0;
                for i in 0..first.data.len() {
                    let mut p = 1.0;
                    for g in &grids {
                        match &g.data[i] {
                            Value::Number(x) => p *= x,
                            Value::Error(k) => return Err(*k),
                            _ => p = 0.0,
                        }
                    }
                    total += p;
                }
                Value::Number(total)
            }
            VLookup | HLookup => {
                need(3, 4)?;
                let key = self.scalar(&a[0]);
                if let Value::Error(k) = key {
                    return Err(k);
                }
                let t = self.tab(&a[1])?;
                let idx = self.num(&a[2])?.trunc() as i64;
                // Written but empty (VLOOKUP(x,t,2,)) is FALSE, as in Excel.
                let approx = if matches!(a.get(3), Some(Expr::Missing)) { false } else { self.opt_bool(a, 3, true)? };
                let vertical = f == VLookup;
                let (len, width) = if vertical { (t.rows(), t.cols()) } else { (t.cols(), t.rows()) };
                if idx < 1 || idx as usize > width {
                    return Err(if idx < 1 { ErrorKind::Value } else { ErrorKind::Ref });
                }
                let at = |i: usize, j: usize| if vertical { t.get(i, j) } else { t.get(j, i) };
                let pos = lookup_pos(len as u32, |i| at(i as usize, 0), &key, if approx { 1 } else { 0 }, &self.locale)?;
                // An empty cell found stays empty for what uses it
                // (ISBLANK(VLOOKUP(...)), VLOOKUP(...)&""); alone it shows 0.
                at(pos as usize, idx as usize - 1)
            }
            XLookup => {
                need(3, 6)?;
                let key = self.scalar(&a[0]);
                if let Value::Error(k) = key {
                    return Err(k);
                }
                let look = self.tab(&a[1])?;
                let ret = self.tab(&a[2])?;
                let mode = self.opt_num(a, 4, 0.0)? as i32;
                let search = self.opt_num(a, 5, 1.0)? as i32;
                let vertical = look.cols() == 1;
                if !vertical && look.rows() != 1 {
                    return Err(ErrorKind::Value);
                }
                let len = look.len();
                if (vertical && ret.rows() != len) || (!vertical && ret.cols() != len) {
                    return Err(ErrorKind::Value);
                }
                match xmatch(len, |i| look.nth(i), &key, mode, search, &self.locale) {
                    Some(i) => {
                        let m = if vertical {
                            Matrix::new(1, ret.cols(), (0..ret.cols()).map(|c| ret.get(i, c)).collect())
                        } else {
                            Matrix::new(ret.rows(), 1, (0..ret.rows()).map(|r| ret.get(r, i)).collect())
                        };
                        m.into_value()
                    }
                    None => match a.get(3) {
                        Some(Expr::Missing) | None => return Err(ErrorKind::Na),
                        Some(d) => self.eval(d),
                    },
                }
            }
            Match => {
                need(2, 3)?;
                let key = self.scalar(&a[0]);
                if let Value::Error(k) = key {
                    return Err(k);
                }
                let t = self.tab(&a[1])?;
                // Written but empty (MATCH(x,r,)) is 0, as in Excel.
                let mode = if matches!(a.get(2), Some(Expr::Missing)) { 0 } else { self.opt_num(a, 2, 1.0)? as i32 };
                if t.cols() != 1 && t.rows() != 1 {
                    return Err(ErrorKind::Na);
                }
                Value::Number((lookup_pos(t.len() as u32, |i| t.nth(i as usize), &key, mode, &self.locale)? + 1) as f64)
            }
            Index => {
                need(2, 4)?;
                let scalar = |e: Option<&Expr>| !matches!(e.map(|e| self.eval(e)), Some(Value::Array(_)));
                if self.reference(&a[0]).is_some() && scalar(a.get(1)) && scalar(a.get(2)) {
                    let (s, r) = self.ref_call(Index, a).unwrap()?;
                    return Ok(self.ref_value(s, &r));
                }
                let t = self.tab(&a[0])?;
                let mut row = self.num(&a[1])?.trunc() as i64;
                let mut col = self.opt_num(a, 2, 0.0)?.trunc() as i64;
                if n == 2 && t.rows() == 1 {
                    col = row;
                    row = 1;
                }
                if row < 0 || col < 0 || row as usize > t.rows() || col as usize > t.cols() {
                    return Err(ErrorKind::Ref);
                }
                let m = t.to_matrix()?;
                let rows: Vec<usize> = if row == 0 { (0..m.rows).collect() } else { vec![row as usize - 1] };
                let cols: Vec<usize> = if col == 0 && m.cols > 1 { (0..m.cols).collect() } else { vec![(col.max(1)) as usize - 1] };
                let data = rows.iter().flat_map(|&r| cols.iter().map(move |&c| (r, c))).map(|(r, c)| m.get(r, c).clone()).collect();
                match Matrix::new(rows.len(), cols.len(), data).into_value() {
                    Value::Empty => Value::Number(0.0),
                    v => v,
                }
            }
            Choose => {
                if n < 2 {
                    return Err(ErrorKind::Value);
                }
                let i = self.num(&a[0])?.trunc() as usize;
                if i < 1 || i >= n {
                    return Err(ErrorKind::Value);
                }
                self.eval(&a[i])
            }
            Row | Column => {
                let r = match a.first() {
                    None | Some(Expr::Missing) => Range::single(self.cell),
                    Some(e) => self.range_arg(e)?.1,
                };
                if f == Row && r.rows() > 1 {
                    let data = (r.start.row..=r.end.row).map(|x| Value::Number(x as f64 + 1.0)).collect();
                    Value::Array(Arc::new(Matrix::new(r.rows() as usize, 1, data)))
                } else if f == Column && r.cols() > 1 {
                    let data = (r.start.col..=r.end.col).map(|x| Value::Number(x as f64 + 1.0)).collect();
                    Value::Array(Arc::new(Matrix::new(1, r.cols() as usize, data)))
                } else {
                    Value::Number(if f == Row { r.start.row + 1 } else { r.start.col + 1 } as f64)
                }
            }
            Rows | Columns => {
                need(1, 1)?;
                let (rows, cols) = match &a[0] {
                    Expr::Range { a: x, b: y, .. } => {
                        let r = Range::new(CellRef::new(x.row, x.col), CellRef::new(y.row, y.col));
                        (r.rows() as usize, r.cols() as usize)
                    }
                    e => {
                        let t = self.tab(e)?;
                        (t.rows(), t.cols())
                    }
                };
                Value::Number(if f == Rows { rows } else { cols } as f64)
            }
            Concatenate => {
                let mut s = String::new();
                for e in a {
                    s.push_str(&self.text(e)?);
                }
                Value::text(s)
            }
            Concat => {
                let mut s = String::new();
                for e in a {
                    let mut err = None;
                    self.each(e, &mut |v, _| match v.to_text() {
                        Ok(t) if s.len() <= MAX_TEXT_BYTES => s.push_str(&t),
                        Ok(_) => {}
                        Err(k) => {
                            err.get_or_insert(k);
                        }
                    });
                    if let Some(k) = err {
                        return Err(k);
                    }
                }
                if s.len() > MAX_TEXT_BYTES {
                    return Err(ErrorKind::Value);
                }
                Value::text(s)
            }
            TextJoin => {
                if n < 3 {
                    return Err(ErrorKind::Value);
                }
                let sep = self.text(&a[0])?;
                let skip_empty = self.boolean(&a[1])?;
                let mut parts: Vec<Arc<str>> = Vec::new();
                for e in &a[2..] {
                    let mut err = None;
                    match self.arg(e) {
                        Arg::R(_, r) if r.cell_count() as usize > MAX_ARRAY => return Err(ErrorKind::Num),
                        Arg::R(sh, r) => {
                            // Dense: empty cells count unless skipped.
                            for row in r.start.row..=r.end.row {
                                for col in r.start.col..=r.end.col {
                                    if !tick() {
                                        return Err(ErrorKind::Num);
                                    }
                                    match self.ctx.value(sh, CellRef::new(row, col)).to_text() {
                                        Ok(t) => parts.push(t),
                                        Err(k) => return Err(k),
                                    }
                                }
                            }
                        }
                        _ => self.each(e, &mut |v, _| match v.to_text() {
                            Ok(t) => parts.push(t),
                            Err(k) => {
                                err.get_or_insert(k);
                            }
                        }),
                    }
                    if let Some(k) = err {
                        return Err(k);
                    }
                }
                if skip_empty {
                    parts.retain(|p| !p.is_empty());
                }
                // Too long a result is #VALUE!: before joining, not after.
                if parts.iter().map(|p| p.len() + sep.len()).sum::<usize>() > MAX_TEXT_BYTES + sep.len() {
                    return Err(ErrorKind::Value);
                }
                Value::text(parts.join(&*sep))
            }
            Len => Value::Number(self.text1(a)?.chars().count() as f64),
            Upper => Value::text(self.text1(a)?.to_uppercase()),
            Lower => Value::text(self.text1(a)?.to_lowercase()),
            Proper => {
                let mut out = String::new();
                let mut prev_alpha = false;
                for c in self.text1(a)?.chars() {
                    if c.is_alphabetic() {
                        if prev_alpha { out.extend(c.to_lowercase()) } else { out.extend(c.to_uppercase()) }
                        prev_alpha = true;
                    } else {
                        out.push(c);
                        prev_alpha = false;
                    }
                }
                Value::text(out)
            }
            Trim => Value::text(self.text1(a)?.split_whitespace().collect::<Vec<_>>().join(" ")),
            Left | Right => {
                need(1, 2)?;
                let s = self.text(&a[0])?;
                let k = self.opt_num(a, 1, 1.0)?;
                if k < 0.0 {
                    return Err(ErrorKind::Value);
                }
                let k = k as usize;
                let chars: Vec<char> = s.chars().collect();
                let k = k.min(chars.len());
                Value::text(if f == Left { chars[..k].iter().collect::<String>() } else { chars[chars.len() - k..].iter().collect() })
            }
            Mid => {
                need(3, 3)?;
                let s = self.text(&a[0])?;
                let start = self.num(&a[1])?;
                let len = self.num(&a[2])?;
                if start < 1.0 || len < 0.0 {
                    return Err(ErrorKind::Value);
                }
                Value::text(s.chars().skip(start as usize - 1).take(len as usize).collect::<String>())
            }
            Find | Search => {
                need(2, 3)?;
                let needle = self.text(&a[0])?;
                let hay = self.text(&a[1])?;
                let start = self.opt_num(a, 2, 1.0)? as usize;
                if start < 1 {
                    return Err(ErrorKind::Value);
                }
                let hay_chars: Vec<char> = hay.chars().collect();
                let (h, nd): (Vec<char>, Vec<char>) = if f == Search {
                    (hay_chars.iter().flat_map(|c| c.to_lowercase()).collect(), needle.chars().flat_map(|c| c.to_lowercase()).collect())
                } else {
                    (hay_chars, needle.chars().collect())
                };
                if start > h.len() + 1 {
                    return Err(ErrorKind::Value);
                }
                // SEARCH takes the wildcards ? and * (~ before them for themselves).
                let pos = if f == Search && nd.iter().any(|c| matches!(c, '?' | '*' | '~')) {
                    let p: Vec<char> = nd.iter().copied().chain(std::iter::once('*')).collect();
                    // A leading * matches from the start or nowhere; the
                    // tries share one budget (past it, no match).
                    let last = if p[0] == '*' { start - 1 } else { h.len() };
                    let budget = wild_budget(1 << 25);
                    let mut work = budget;
                    let pos = (start - 1..=last).find(|&i| wildcard_in(&p, &h[i..], &mut work));
                    wild_spent(budget - work);
                    pos
                } else {
                    (start - 1..=h.len().saturating_sub(nd.len())).find(|&i| h[i..].starts_with(&nd))
                };
                match pos {
                    Some(p) => Value::Number((p + 1) as f64),
                    None => return Err(ErrorKind::Value),
                }
            }
            Substitute => {
                need(3, 4)?;
                let s = self.text(&a[0])?;
                let old = self.text(&a[1])?;
                let new = self.text(&a[2])?;
                if old.is_empty() {
                    return Ok(Value::Text(s));
                }
                match a.get(3) {
                    None => {
                        let grows = new.chars().count().saturating_sub(old.chars().count()).saturating_mul(s.matches(&*old).count());
                        if grows > 0 && s.chars().count().saturating_add(grows) > MAX_TEXT {
                            return Err(ErrorKind::Value);
                        }
                        Value::text(s.replace(&*old, &new))
                    }
                    Some(e) => {
                        let nth = self.num(e)? as usize;
                        match s.match_indices(&*old).nth(nth.saturating_sub(1)) {
                            Some((i, _)) if nth >= 1 => Value::text(format!("{}{}{}", &s[..i], new, &s[i + old.len()..])),
                            _ => Value::Text(s),
                        }
                    }
                }
            }
            Replace => {
                need(4, 4)?;
                let s: Vec<char> = self.text(&a[0])?.chars().collect();
                let start = (self.num(&a[1])? as usize).max(1) - 1;
                let len = self.num(&a[2])? as usize;
                let new = self.text(&a[3])?;
                let start = start.min(s.len());
                let end = start.saturating_add(len).min(s.len());
                Value::text(format!("{}{}{}", s[..start].iter().collect::<String>(), new, s[end..].iter().collect::<String>()))
            }
            Rept => {
                need(2, 2)?;
                let k = self.num(&a[1])?;
                if !(0.0..=32767.0).contains(&k) {
                    return Err(ErrorKind::Value);
                }
                let t = self.text(&a[0])?;
                // Checked before the text is built: nested REPTs would ask for terabytes.
                if t.chars().count().saturating_mul(k as usize) > MAX_TEXT {
                    return Err(ErrorKind::Value);
                }
                Value::text(t.repeat(k as usize))
            }
            Exact => {
                need(2, 2)?;
                Value::Bool(self.text(&a[0])? == self.text(&a[1])?)
            }
            Text => {
                need(2, 2)?;
                let v = self.scalar(&a[0]);
                let code = self.text(&a[1])?;
                let v = match v {
                    Value::Text(t) => match format::parse_number(&t, &self.locale) {
                        Some(n) => Value::Number(n),
                        None => Value::Text(t),
                    },
                    v => v,
                };
                Value::text(NumFormat::parse(&code).format(&v, &self.locale).0)
            }
            ValueFn => match self.scalar(&a.first().cloned().unwrap_or(Expr::Missing)) {
                Value::Number(x) => Value::Number(x),
                Value::Text(t) => match format::parse_input(&t, &self.locale).value {
                    Value::Number(x) => Value::Number(x),
                    _ => return Err(ErrorKind::Value),
                },
                Value::Error(k) => return Err(k),
                _ => return Err(ErrorKind::Value),
            },
            Char => {
                let c = self.num1(a)? as u32;
                Value::text(char::from_u32(c).filter(|_| c >= 1).ok_or(ErrorKind::Value)?.to_string())
            }
            Code => Value::Number(self.text1(a)?.chars().next().ok_or(ErrorKind::Value)? as u32 as f64),
            IsBlank => {
                need(1, 1)?;
                Value::Bool(self.scalar_raw(&a[0]).is_empty())
            }
            IsNumber => {
                need(1, 1)?;
                Value::Bool(matches!(self.scalar_raw(&a[0]), Value::Number(_)))
            }
            IsText => Value::Bool(matches!(self.eval1(a)?, Value::Text(_))),
            IsError => Value::Bool(matches!(self.eval1(a)?, Value::Error(_))),
            IsNa => Value::Bool(matches!(self.eval1(a)?, Value::Error(ErrorKind::Na))),
            Na => return Err(ErrorKind::Na),
            Today => Value::Number(format::now_serial().floor()),
            Now => Value::Number(format::now_serial()),
            Date => {
                need(3, 3)?;
                let (y, m, d) = (self.num(&a[0])?.trunc() as i64, self.num(&a[1])?.trunc() as i64, self.num(&a[2])?.trunc() as i64);
                let y = if y < 1900 { y + 1900 } else { y };
                if !(0..10_000).contains(&y) {
                    return Err(ErrorKind::Num);
                }
                let s = format::excel_serial(y, m, d);
                if s < 0.0 {
                    return Err(ErrorKind::Num);
                }
                Value::Number(s)
            }
            Year | Month | Day => {
                let x = self.num1(a)?;
                if x < 0.0 {
                    return Err(ErrorKind::Num);
                }
                let (y, m, d) = format::excel_date(x);
                Value::Number(match f {
                    Year => y as f64,
                    Month => m as f64,
                    _ => d as f64,
                })
            }
            Hour | Minute | Second => {
                let x = self.num1(a)?;
                let secs = ((x - x.floor()) * 86_400.0).round() as i64;
                Value::Number(match f {
                    Hour => secs / 3600 % 24,
                    Minute => secs / 60 % 60,
                    _ => secs % 60,
                } as f64)
            }
            Weekday => {
                need(1, 2)?;
                let x = self.num(&a[0])?.floor() as i64;
                let t = self.opt_num(a, 1, 1.0)? as i64;
                let sun0 = (x - 1).rem_euclid(7); // 0 = Sunday
                let mon0 = (sun0 + 6) % 7; // 0 = Monday
                Value::Number(match t {
                    1 => (sun0 + 1) as f64,
                    2 => (mon0 + 1) as f64,
                    3 => mon0 as f64,
                    11..=17 => ((mon0 - (t - 11)).rem_euclid(7) + 1) as f64,
                    _ => return Err(ErrorKind::Num),
                })
            }
            EDate | EoMonth => {
                need(2, 2)?;
                let x = self.num(&a[0])?;
                if x < 0.0 {
                    return Err(ErrorKind::Num);
                }
                let (y, m, d) = format::excel_date(x);
                let k = self.num(&a[1])?.trunc() as i64;
                // The month's real length (no 29 February 1900 here).
                let months = y * 12 + m as i64 - 1 + k;
                let (y2, m2) = (months.div_euclid(12), (months.rem_euclid(12) + 1) as u32);
                let ld = date::month_len(y2, m2);
                let r = format::excel_serial(y2, m2 as i64, if f == EDate { d.min(ld) } else { ld } as i64);
                if r < 0.0 {
                    return Err(ErrorKind::Num);
                }
                Value::Number(r)
            }
            Days => {
                need(2, 2)?;
                Value::Number((self.num(&a[0])?.floor() - self.num(&a[1])?.floor()) as f64)
            }
            Pmt | Fv | Pv => {
                need(3, 5)?;
                let rate = self.num(&a[0])?;
                let nper = self.num(&a[1])?;
                let x = self.num(&a[2])?;
                let y = self.opt_num(a, 3, 0.0)?;
                // Any nonzero type means payments at the start of the period.
                let typ = if self.opt_num(a, 4, 0.0)? != 0.0 { 1.0 } else { 0.0 };
                let r = match f {
                    Pmt => {
                        let (pv, fv) = (x, y);
                        if rate == 0.0 {
                            -(pv + fv) / nper
                        } else {
                            let q = (1.0 + rate).powf(nper);
                            -(rate * (pv * q + fv)) / ((1.0 + rate * typ) * (q - 1.0))
                        }
                    }
                    Fv => {
                        let (pmt, pv) = (x, y);
                        if rate == 0.0 {
                            -(pv + pmt * nper)
                        } else {
                            let q = (1.0 + rate).powf(nper);
                            -(pv * q + pmt * (1.0 + rate * typ) * (q - 1.0) / rate)
                        }
                    }
                    _ => {
                        let (pmt, fv) = (x, y);
                        if rate == 0.0 {
                            -(fv + pmt * nper)
                        } else {
                            let q = (1.0 + rate).powf(nper);
                            -(fv + pmt * (1.0 + rate * typ) * (q - 1.0) / rate) / q
                        }
                    }
                };
                num_ok(r)
            }
            Npv => {
                if n < 2 {
                    return Err(ErrorKind::Value);
                }
                let rate = self.num(&a[0])?;
                if rate == -1.0 {
                    return Err(ErrorKind::Div0);
                }
                let mut v = Vec::new();
                self.numbers(&a[1..], &mut v)?;
                num_ok(v.iter().enumerate().map(|(i, x)| x / (1.0 + rate).powi(i as i32 + 1)).sum())
            }
            _ => {
                return math::call(self, f, a)
                    .or_else(|| complex::call(self, f, a))
                    .or_else(|| stats::call(self, f, a))
                    .or_else(|| text::call(self, f, a))
                    .or_else(|| text2::call(self, f, a))
                    .or_else(|| web::call(self, f, a))
                    .or_else(|| date::call(self, f, a))
                    .or_else(|| array::call(self, f, a))
                    .or_else(|| database::call(self, f, a))
                    .or_else(|| groupby::call(self, f, a))
                    .or_else(|| google::call(self, f, a))
                    .or_else(|| finance::call(self, f, a))
                    .or_else(|| securities::call(self, f, a))
                    .or_else(|| eng::call(self, f, a))
                    .unwrap_or(Err(ErrorKind::Name));
            }
        })
    }

    pub(crate) fn num1(&self, a: &[Expr]) -> R<f64> {
        if a.len() != 1 {
            return Err(ErrorKind::Value);
        }
        self.num(&a[0])
    }

    pub(crate) fn text1(&self, a: &[Expr]) -> R<Arc<str>> {
        if a.len() != 1 {
            return Err(ErrorKind::Value);
        }
        self.text(&a[0])
    }

    pub(crate) fn eval1(&self, a: &[Expr]) -> R<Value> {
        if a.len() != 1 {
            return Err(ErrorKind::Value);
        }
        Ok(self.scalar(&a[0]))
    }
}

/// Finds the index of `key` in a 1-D sequence. `mode`: 0 exact,
/// 1 largest <= key (sorted ascending), -1 smallest >= key (sorted descending).
pub(crate) fn lookup_pos(len: u32, get: impl Fn(u32) -> Value, key: &Value, mode: i32, loc: &Locale) -> R<u32> {
    if mode == 0 {
        let crit = Criteria::exact(key.clone(), loc);
        return (0..len).find(|&i| crit.matches(&get(i))).ok_or(ErrorKind::Na);
    }
    // A binary search, as Excel does: on unsorted data the answer is
    // Excel's, not the first run a scan would find. Values of another type
    // (errors, empty cells) are stepped over towards the end of the half.
    let past = if mode > 0 { Ordering::Greater } else { Ordering::Less };
    let (mut lo, mut hi) = (0i64, len as i64 - 1);
    let mut best = None;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let found = (mid..=hi).find_map(|m| {
            let v = get(m as u32);
            (std::mem::discriminant(&v) == std::mem::discriminant(key)).then(|| (m, v.compare(key)))
        });
        match found {
            Some((m, ord)) if ord != past => {
                best = Some(m as u32);
                lo = m + 1;
            }
            _ => hi = mid - 1,
        }
    }
    best.ok_or(ErrorKind::Na)
}

/// XLOOKUP/XMATCH search. `mode`: 0 exact, -1 exact or next smaller,
/// 1 exact or next larger, 2 wildcards. `search`: 1 first to last, -1
/// last to first, ±2 binary (searched linearly: same answer on sorted data).
pub(crate) fn xmatch(len: usize, get: impl Fn(usize) -> Value, key: &Value, mode: i32, search: i32, loc: &Locale) -> Option<usize> {
    let order: Box<dyn Iterator<Item = usize>> = if search < 0 { Box::new((0..len).rev()) } else { Box::new(0..len) };
    let crit = if mode == 2 { Criteria::exact(key.clone(), loc) } else { Criteria { op: CritOp::Eq, value: key.clone(), pattern: None, numeric_text: None } };
    let mut best: Option<(usize, Value)> = None;
    for i in order {
        let v = get(i);
        if crit.matches(&v) {
            return Some(i);
        }
        if mode == 1 || mode == -1 {
            let same = std::mem::discriminant(&v) == std::mem::discriminant(key);
            if !same {
                continue;
            }
            let ord = v.compare(key);
            let wanted = if mode == 1 { Ordering::Greater } else { Ordering::Less };
            if ord == wanted {
                let better = match &best {
                    None => true,
                    Some((_, b)) => v.compare(b) == wanted.reverse(),
                };
                if better {
                    best = Some((i, v));
                }
            }
        }
    }
    best.map(|b| b.0)
}

/// `R1C1`, `R[-1]C[2]`, `RC`, with an optional sheet and a second corner.
fn parse_r1c1(s: &str, at: CellRef) -> Option<Expr> {
    let (sheet, body) = match s.rsplit_once('!') {
        Some((sh, b)) => (Some(Arc::from(sh.trim_matches('\''))), b),
        None => (None, s),
    };
    let one = |t: &str| -> Option<CellAddr> {
        let t = t.to_ascii_uppercase();
        let t = t.strip_prefix('R')?;
        let (rp, cp) = t.split_once('C')?;
        let part = |p: &str, base: u32| -> Option<(u32, bool)> {
            if p.is_empty() {
                Some((base, false))
            } else if let Some(x) = p.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
                let v = base as i64 + x.parse::<i64>().ok()?;
                (v >= 0).then_some((v as u32, false))
            } else {
                let v = p.parse::<u32>().ok()?;
                (v >= 1).then_some((v - 1, true))
            }
        };
        let (row, row_abs) = part(rp, at.row)?;
        let (col, col_abs) = part(cp, at.col)?;
        (row < MAX_ROWS && col < MAX_COLS).then_some(CellAddr { row, col, row_abs, col_abs })
    };
    match body.split_once(':') {
        Some((x, y)) => Some(Expr::Range { sheet, a: one(x)?, b: one(y)?, kind: RangeKind::Cells }),
        None => Some(Expr::Ref { sheet, addr: one(body)? }),
    }
}

// ---------------------------------------------------------------------------
// Criteria for *IF functions: ">5", "<>x", "a*", "=", 10 ...

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CritOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

pub struct Criteria {
    op: CritOp,
    value: Value,
    pattern: Option<Vec<char>>,
    /// *IF criteria: a text cell that reads as a number equals that
    /// number (COUNTIF(A:A,"007") counts the text "007" and 7).
    numeric_text: Option<Locale>,
}

impl Criteria {
    pub(crate) fn exact(v: Value, _loc: &Locale) -> Self {
        let pattern = match &v {
            Value::Text(t) if t.contains(['*', '?']) => Some(t.chars().flat_map(char::to_lowercase).collect()),
            _ => None,
        };
        Criteria { op: CritOp::Eq, value: v, pattern, numeric_text: None }
    }

    /// Criteria from an operator and an operand typed as text (`abc*`, `5`).
    pub fn with_op(op: CritOp, text: &str, loc: &Locale) -> Self {
        let prefix = match op {
            CritOp::Eq => "=",
            CritOp::Ne => "<>",
            CritOp::Lt => "<",
            CritOp::Le => "<=",
            CritOp::Gt => ">",
            CritOp::Ge => ">=",
        };
        Criteria::new(Value::text(format!("{prefix}{text}")), loc)
    }

    pub fn new(v: Value, loc: &Locale) -> Self {
        let Value::Text(t) = &v else {
            return Criteria { op: CritOp::Eq, value: v, pattern: None, numeric_text: Some(*loc) };
        };
        let (op, rest) = if let Some(r) = t.strip_prefix("<=") {
            (CritOp::Le, r)
        } else if let Some(r) = t.strip_prefix(">=") {
            (CritOp::Ge, r)
        } else if let Some(r) = t.strip_prefix("<>") {
            (CritOp::Ne, r)
        } else if let Some(r) = t.strip_prefix('<') {
            (CritOp::Lt, r)
        } else if let Some(r) = t.strip_prefix('>') {
            (CritOp::Gt, r)
        } else if let Some(r) = t.strip_prefix('=') {
            (CritOp::Eq, r)
        } else {
            (CritOp::Eq, &t[..])
        };
        let value = if rest.is_empty() {
            Value::Empty
        } else {
            match format::parse_input(rest, loc).value {
                Value::Text(_) => Value::text(rest),
                x => x,
            }
        };
        let pattern = match &value {
            Value::Text(t) if t.contains(['*', '?']) && matches!(op, CritOp::Eq | CritOp::Ne) => Some(t.chars().flat_map(char::to_lowercase).collect()),
            _ => None,
        };
        Criteria { op, value, pattern, numeric_text: Some(*loc) }
    }

    pub fn matches(&self, v: &Value) -> bool {
        if let Some(p) = &self.pattern {
            let hit = match v {
                Value::Text(t) => wildcard(p, &t.chars().flat_map(char::to_lowercase).collect::<Vec<_>>()),
                _ => false,
            };
            return if matches!(self.op, CritOp::Ne) { !hit } else { hit };
        }
        if matches!(self.value, Value::Empty) {
            let blank = v.is_empty() || matches!(v, Value::Text(t) if t.is_empty());
            return match self.op {
                CritOp::Ne => !blank,
                _ => blank,
            };
        }
        if let (Value::Number(_), Value::Text(t), Some(loc), CritOp::Eq | CritOp::Ne) = (&self.value, v, &self.numeric_text, self.op) {
            if let Value::Number(n) = format::parse_input(t, loc).value {
                return self.matches(&Value::Number(n));
            }
        }
        let comparable = match (&self.value, v) {
            (Value::Number(_), Value::Number(_)) => true,
            (Value::Text(_), Value::Text(_)) => true,
            (Value::Bool(_), Value::Bool(_)) => true,
            _ => false,
        };
        if !comparable {
            return matches!(self.op, CritOp::Ne);
        }
        let ord = match (v, &self.value) {
            (Value::Text(a), Value::Text(b)) => cmp_text_ci(a, b),
            _ => v.compare(&self.value),
        };
        match self.op {
            CritOp::Eq => ord == Ordering::Equal,
            CritOp::Ne => ord != Ordering::Equal,
            CritOp::Lt => ord == Ordering::Less,
            CritOp::Le => ord != Ordering::Greater,
            CritOp::Gt => ord == Ordering::Greater,
            CritOp::Ge => ord != Ordering::Less,
        }
    }
}

/// `*`, `?` and `~` as the criteria of COUNTIF and Find match them.
pub fn wildcard(p: &[char], s: &[char]) -> bool {
    let budget = wild_budget(1 << 24);
    let mut work = budget;
    let r = wildcard_in(p, s, &mut work);
    wild_spent(budget - work);
    r
}

/// `wildcard`, taking its steps from `work`: past it, no match (a long
/// pattern over a long text takes their product).
fn wildcard_in(p: &[char], s: &[char], work: &mut usize) -> bool {
    // Iterative glob matching with backtracking on the last '*'.
    let (mut pi, mut si) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while si < s.len() {
        if *work == 0 {
            return false;
        }
        *work -= 1;
        if pi < p.len() && p[pi] == '~' && pi + 1 < p.len() && p[pi + 1] == s[si] {
            pi += 2;
            si += 1;
        } else if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn rand01() -> f64 {
    use std::cell::Cell;
    thread_local!(static STATE: Cell<u64> = Cell::new(0));
    STATE.with(|s| {
        let mut x = s.get();
        if x == 0 {
            x = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E37_79B9_7F4A_7C15) | 1;
        }
        // xorshift64*
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        s.set(x);
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    })
}
