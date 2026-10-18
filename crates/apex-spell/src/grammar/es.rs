//! Spanish rules. What a word is comes from the Spanish dictionaries of
//! LibreOffice (RLA-ES): S makes plurals, G the feminine ("niño" ->
//! "niña"), R, E, I and X conjugate verbs. A noun's own gender isn't in
//! them: it's guessed from its ending, with the exceptions listed here.

use super::{Ctx, Kind, case_like};
use crate::Analysis;

const VERB: &[&str] = &["R", "E", "I", "X"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum G {
    M,
    F,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum N {
    Sing,
    Plur,
    /// "lunes", "crisis".
    Both,
}

/// Determiners: masculine and feminine singular, then plural.
const DETS: &[[&str; 4]] = &[
    ["el", "la", "los", "las"],
    ["un", "una", "unos", "unas"],
    ["este", "esta", "estos", "estas"],
    ["ese", "esa", "esos", "esas"],
    ["aquel", "aquella", "aquellos", "aquellas"],
    ["algún", "alguna", "algunos", "algunas"],
    ["ningún", "ninguna", "ningunos", "ningunas"],
    ["nuestro", "nuestra", "nuestros", "nuestras"],
    ["vuestro", "vuestra", "vuestros", "vuestras"],
    ["del", "de la", "de los", "de las"],
    ["al", "a la", "a los", "a las"],
    ["mi", "mi", "mis", "mis"],
    ["tu", "tu", "tus", "tus"],
    ["su", "su", "sus", "sus"],
];

/// Also pronouns before a verb: "las casa" is "he marries them", "estos
/// son" "these are".
const CLITIC: &[&str] = &["la", "las", "los", "este", "esta", "estos", "estas", "ese", "esa", "esos", "esas", "aquel", "aquella", "aquellos", "aquellas"];

const PREPOSITIONS: &[&str] = &["a", "ante", "bajo", "con", "contra", "de", "desde", "durante", "en", "entre", "hacia", "hasta", "mediante", "para", "por", "según", "sin", "sobre", "tras"];

/// Masculine nouns ending in -a.
const MASC_A: &[&str] = &[
    "día", "mapa", "problema", "tema", "sistema", "programa", "idioma", "clima", "drama", "esquema", "poema", "dilema", "enigma", "lema", "planeta", "panorama", "diagrama", "dogma", "fantasma", "prisma", "síntoma",
    "telegrama", "teorema", "aroma", "carisma", "crucigrama", "emblema", "estigma", "holograma", "pijama", "tranvía", "mediodía", "tequila", "karma", "magma", "trauma", "axioma", "cisma", "diploma", "ecosistema", "fonema",
    "morfema", "lexema", "pentagrama", "organigrama", "anagrama", "epigrama", "monograma", "yoga", "koala", "gorila", "puma", "vodka", "sida", "alfa", "beta", "delta", "gamma", "omega", "sofá", "subsistema", "dramaturgo",
    "climograma", "cronograma", "histograma", "radiograma", "programa", "reuma", "plasma", "edema", "eccema", "enema", "glaucoma", "carcinoma", "melanoma", "hematoma", "sarcoma", "papá", "bebá", "maná", "sofá",
];

/// Feminine nouns ending in -o.
const FEM_O: &[&str] = &["mano", "foto", "moto", "libido", "seo", "nao", "dinamo", "dínamo", "polio", "radio", "virago", "porno"];

/// Nouns of both genders, or that change meaning with it.
const BOTH: &[&str] = &[
    "cometa", "cura", "papa", "orden", "frente", "capital", "cólera", "margen", "mar", "arte", "radio", "pendiente", "guía", "guardia", "policía", "editorial", "corte", "final", "terminal", "clave", "modelo", "testigo",
    "soprano", "reo", "atleta", "idiota", "patriota", "compatriota", "homicida", "suicida", "colega", "camarada", "espía", "centinela", "hipócrita", "analfabeta", "nómada", "pediatra", "psiquiatra", "geriatra", "logopeda",
    "terapeuta", "fisioterapeuta", "burócrata", "demócrata", "aristócrata", "astronauta", "internauta", "cosmonauta", "psicópata", "agente", "cliente", "estudiante", "pianista", "joven", "bebé", "mártir", "rehén", "cónyuge",
    "intérprete", "líder", "chef", "piloto", "doble", "azúcar", "calor", "color", "lente", "tilde", "interrogante", "dote", "pringue", "maratón", "sartén", "mimbre", "poeta", "pirata", "profeta", "escriba", "monarca",
    "patriarca", "jerarca", "oligarca", "cineasta", "entusiasta", "gimnasta", "iconoclasta", "panda", "maya", "quechua", "aimara", "aymara", "vigía", "recluta", "lama", "canalla", "estratega", "belga", "celta", "persa",
    "azteca", "inca", "israelita", "jesuita", "cosmopolita", "miembro", "rosa", "violeta", "naranja", "lila", "malva", "coma", "macro", "disco", "anterior",
    "posterior", "superior", "inferior", "mayor", "menor", "mejor", "peor", "exterior", "interior", "ulterior",
];

/// Numbers: "las once", "los cuatro".
const NUMERALS: &[&str] = &[
    "uno", "una", "dos", "tres", "cuatro", "cinco", "seis", "siete", "ocho", "nueve", "diez", "once", "doce", "trece", "catorce", "quince", "dieciséis", "diecisiete", "dieciocho", "diecinueve", "veinte", "treinta",
    "cuarenta", "cincuenta", "sesenta", "setenta", "ochenta", "noventa", "cien", "ciento", "mil", "media",
];

/// Feminine nouns that start with a stressed "a": "el agua", "un hacha".
const STRESSED_A: &[&str] = &[
    "agua", "águila", "alma", "arma", "ala", "aula", "área", "ave", "hambre", "hacha", "hada", "habla", "ancla", "asa", "ascua", "haba", "hampa", "alba", "alga", "ansia", "arca", "arpa", "asma", "aspa", "haya", "hache",
    "ama", "ánima", "ánfora", "álgebra", "acta", "alza", "aria", "aya", "ágata", "ánade", "áspid", "ápside", "arte", "habla", "asta", "ara",
];

/// "-ión" nouns that are masculine.
const MASC_ION: &[&str] = &["avión", "camión", "gorrión", "sarampión", "aluvión", "alción", "anfitrión", "centurión", "embrión", "escorpión", "guion", "guión", "ión", "limpión", "rebullón", "turión", "bastión", "histrión", "pión", "quion"];

/// The noun's gender from its ending, when it's safe to say.
fn gender_of(root: &str) -> Option<G> {
    if BOTH.contains(&root) {
        return None;
    }
    if MASC_A.contains(&root) || MASC_ION.contains(&root) {
        return Some(G::M);
    }
    if FEM_O.contains(&root) {
        return Some(G::F);
    }
    for end in ["ista", "cida", "crata", "nauta", "iatra", "peuta", "ópata", "sis"] {
        if root.ends_with(end) {
            return None;
        }
    }
    if root.ends_with("ión") || root.ends_with("dad") || root.ends_with("tad") || root.ends_with("tud") || root.ends_with("umbre") || root.ends_with("itis") {
        return Some(G::F);
    }
    if root.ends_with("aje") || (root.ends_with("or") && !matches!(root, "flor" | "labor" | "coliflor" | "sor")) {
        return Some(G::M);
    }
    if root.ends_with('a') {
        return Some(G::F);
    }
    if root.ends_with('o') {
        return Some(G::M);
    }
    None
}

/// What a word after a determiner is: a noun or adjective (its gender and
/// number) and whether it is also a verb form.
struct Nominal {
    g: Option<G>,
    n: N,
    /// The singular, for "los problema" -> no; for "el casas" -> "casa".
    singular: Option<String>,
    verb: bool,
}

/// A stem with a feminine (G): masculine if it ends in -o ("niño"); "la
/// presidente" or "la jefe" are also said.
fn masc_of_g(root: &str) -> Option<G> {
    root.ends_with('o').then_some(G::M)
}

fn is_verb(a: &Analysis) -> bool {
    match a.last_affix() {
        Some(x) => VERB.contains(&x),
        // An infinitive.
        None => VERB.iter().any(|f| a.has_flag(f)) && (a.root.ends_with("ar") || a.root.ends_with("er") || a.root.ends_with("ir") || a.root.ends_with("ír")),
    }
}

fn nominal(word: &str, all: &[Analysis]) -> Option<Nominal> {
    let verb = all.iter().any(is_verb);
    let mut out: Option<Nominal> = None;
    for a in all {
        let (g, n, singular) = match a.affixes.iter().map(String::as_str).collect::<Vec<_>>()[..] {
            [] if a.has_flag("S") || a.has_flag("G") => {
                let n = if word.ends_with('s') || word.ends_with('x') { N::Both } else { N::Sing };
                let g = if a.has_flag("G") { masc_of_g(&a.root) } else { gender_of(&a.root) };
                (g, n, Some(a.root.clone()))
            }
            [] if !a.has_flag("S") && (a.root.ends_with('s') || a.root.ends_with('x')) && !VERB.iter().any(|f| a.has_flag(f)) => (gender_of(&a.root), N::Both, None),
            ["S"] => (if a.has_flag("G") { masc_of_g(&a.root) } else { gender_of(&a.root) }, N::Plur, Some(a.root.clone())),
            ["G"] => (Some(G::F), if word.ends_with('s') { N::Plur } else { N::Sing }, word.strip_suffix('s').map(str::to_string)),
            ["G", "S"] => (Some(G::F), N::Plur, word.strip_suffix('s').map(str::to_string)),
            _ => continue,
        };
        match &mut out {
            None => out = Some(Nominal { g, n, singular, verb }),
            // Homonyms that disagree: say nothing of what they disagree on.
            Some(o) => {
                if o.g != g {
                    o.g = None;
                }
                if o.n != n {
                    o.n = N::Both;
                }
            }
        }
    }
    out
}

/// The determiner's row, gender and number.
fn det(w: &str) -> Option<(usize, Option<G>, N)> {
    for (k, row) in DETS.iter().enumerate() {
        if let Some(i) = row.iter().position(|x| *x == w) {
            let g = if row[0] == row[1] { None } else if i % 2 == 0 { Some(G::M) } else { Some(G::F) };
            return Some((k, g, if i < 2 { N::Sing } else { N::Plur }));
        }
    }
    None
}

/// The determiner of a row for a noun.
fn det_for(row: usize, g: G, n: N, noun: &str) -> &'static str {
    let r = &DETS[row];
    // "el agua", "un hacha": not "este agua".
    if g == G::F && n == N::Sing && STRESSED_A.contains(&noun) && matches!(r[0], "el" | "un" | "algún" | "ningún" | "del" | "al") {
        return r[0];
    }
    r[match (g, n) {
        (G::M, N::Plur) => 2,
        (G::F, N::Plur) => 3,
        (G::F, _) => 1,
        _ => 0,
    }]
}

/// The plural of a singular noun, as the dictionary's S rule makes it.
fn plural(w: &str) -> String {
    if let Some(h) = w.strip_suffix('z') {
        return format!("{h}ces");
    }
    for (end, plain) in [("án", "anes"), ("én", "enes"), ("ín", "ines"), ("ón", "ones"), ("ún", "unes"), ("ás", "ases"), ("és", "eses"), ("ís", "ises"), ("ós", "oses"), ("ús", "uses")] {
        if let Some(h) = w.strip_suffix(end) {
            return format!("{h}{plain}");
        }
    }
    match w.chars().last() {
        Some('a' | 'e' | 'i' | 'o' | 'u' | 'á' | 'é' | 'ó') => format!("{w}s"),
        // "examen" -> "exámenes" isn't made here: the dictionary turns down
        // the wrong form.
        _ => format!("{w}es"),
    }
}

pub(super) fn check(cx: &mut Ctx) {
    marks(cx);
    for i in 0..cx.toks.len() {
        if cx.toks[i].kind != Kind::Word {
            continue;
        }
        let w = cx.toks[i].lower();
        match w.as_str() {
            "a" | "de" => contraction(cx, i),
            "haber" | "ver" => a_ver(cx, i),
            "sino" | "si" => sino(cx, i),
            "porque" | "porqué" => porque(cx, i),
            _ => {}
        }
        if w == "de" {
            dequeismo(cx, i);
        }
        if det(&w).is_some() {
            agreement(cx, i);
        }
    }
}

/// "a el" -> "al", "de el" -> "del" (not "de El Salvador", nor "de él").
fn contraction(cx: &mut Ctx, i: usize) {
    let Some(j) = cx.next_word(i) else { return };
    if cx.toks[j].s != "el" {
        return;
    }
    let a = cx.toks[i].s;
    let to = case_like(if a.eq_ignore_ascii_case("a") { "al" } else { "del" }, a);
    let r = cx.span(i, j);
    cx.push(r, "es_contraction", vec![to]);
}

/// "haber si" -> "a ver si"; "a ver comido" -> "haber comido".
fn a_ver(cx: &mut Ctx, i: usize) {
    let w = cx.toks[i].lower();
    let Some(j) = cx.next_word(i) else { return };
    if w == "haber" {
        if cx.toks[j].is("si") {
            let to = case_like("a ver", cx.toks[i].s);
            let r = cx.toks[i].r.clone();
            cx.push(r, "es_a_ver", vec![to]);
        }
        return;
    }
    // "a ver" + a participle.
    let Some(p) = cx.prev_word(i) else { return };
    if !cx.toks[p].is("a") {
        return;
    }
    let next = cx.toks[j].lower();
    if !(next.ends_with("ado") || next.ends_with("ido") || next.ends_with("ído") || matches!(next.as_str(), "hecho" | "dicho" | "visto" | "puesto" | "escrito" | "vuelto" | "abierto" | "roto" | "muerto" | "sido")) {
        return;
    }
    let participle = match cx.dict {
        Some(d) => matches!(next.as_str(), "hecho" | "dicho" | "visto" | "puesto" | "escrito" | "vuelto" | "abierto" | "roto" | "muerto" | "sido") || d.analyze(&next).iter().any(|a| a.last_affix() == Some("D")),
        None => true,
    };
    if participle {
        let to = case_like("haber", cx.toks[p].s);
        let r = cx.span(p, i);
        cx.push(r, "es_a_ver", vec![to]);
    }
}

/// Finite forms that the dictionary keeps as stems of their own.
const FINITE: &[&str] = &[
    "es", "soy", "eres", "somos", "son", "era", "fue", "está", "estás", "están", "hay", "he", "has", "ha", "hemos", "han", "voy", "vas", "va", "vamos", "van", "puedo", "puedes", "puede", "pueden", "quiero", "quieres", "quiere",
    "tengo", "tienes", "tiene", "tienen", "sé", "sabes", "sabe", "saben", "vienes", "viene", "vienen", "haces", "hace", "hago", "hacen", "dices", "dice", "digo", "estoy", "haya", "hayan", "sea", "sean", "fuera",
    "fueran", "esté", "estén", "vaya", "vayan",
];

/// Singular nouns that end in -s, that the dictionary may take for plurals.
const INVARIABLE: &[&str] = &["lunes", "martes", "miércoles", "jueves", "viernes", "paraguas", "cumpleaños", "tocadiscos", "sacacorchos", "abrelatas", "parabrisas", "rascacielos", "portaaviones", "salvavidas", "lavavajillas", "virus", "tórax", "análisis", "crisis", "tesis", "síntesis", "dosis", "atlas", "caries"];

const CLITICS: &[&str] = &["me", "te", "se", "lo", "la", "le", "nos", "os", "los", "las", "les"];

/// "sino vienes" -> "si no vienes"; "si no que" -> "sino que".
fn sino(cx: &mut Ctx, i: usize) {
    let w = cx.toks[i].lower();
    let Some(j) = cx.next_word(i) else { return };
    if w == "si" {
        if cx.toks[j].is("no")
            && let Some(k) = cx.next_word(j)
            && cx.toks[k].is("que")
        {
            let to = case_like("sino", cx.toks[i].s);
            let r = cx.span(i, j);
            cx.push(r, "es_sino", vec![to]);
        }
        return;
    }
    // Past the pronouns: "sino lo haces".
    let mut k = j;
    while CLITICS.contains(&cx.toks[k].lower().as_str()) {
        match cx.next_word(k) {
            Some(n) => k = n,
            None => return,
        }
    }
    let next = cx.toks[k].lower();
    let finite = FINITE.contains(&next.as_str())
        || cx.dict.is_some_and(|d| {
            let all = d.analyze(&next);
            // Only a conjugated verb: not a noun ("sino rojo"), nor an
            // infinitive or gerund ("sino dormir", "sino comiendo").
            !all.is_empty() && all.iter().all(|a| VERB.contains(&a.last_affix().unwrap_or(""))) && !next.ends_with("ndo") && !next.ends_with('r')
        });
    if finite {
        let to = case_like("si no", cx.toks[i].s);
        let r = cx.toks[i].r.clone();
        cx.push(r, "es_sino", vec![to]);
    }
}

/// Determiners before "porqué" (the reason).
const BEFORE_PORQUE: &[&str] = &["el", "un", "los", "unos", "su", "sus", "del", "este", "ese", "aquel", "mi", "tu", "cuyo", "cuál", "qué", "ningún", "algún", "otro"];

/// "¿Porque vienes?" -> "¿Por qué vienes?"; "el porque" -> "el porqué".
fn porque(cx: &mut Ctx, i: usize) {
    let w = cx.toks[i].lower();
    let det = cx.prev_word(i).is_some_and(|p| BEFORE_PORQUE.contains(&cx.toks[p].lower().as_str()));
    let asks = i > 0 && cx.toks[i - 1].s == "¿";
    let r = cx.toks[i].r.clone();
    let s = cx.toks[i].s;
    if w == "porque" {
        if asks {
            cx.push(r, "es_porque", vec![case_like("por qué", s)]);
        } else if det {
            cx.push(r, "es_porque", vec!["porqué".into()]);
        }
    } else if !det {
        // "porqué" alone: why (in a question) or because.
        let in_question = cx.text[..cx.toks[i].r.start].rfind(['¿', '.', '?']).is_some_and(|k| cx.text[k..].starts_with('¿'));
        let to = if asks || in_question { vec![case_like("por qué", s), case_like("porque", s)] } else { vec![case_like("porque", s), case_like("por qué", s)] };
        cx.push(r, "es_porque", to);
    }
}

/// Verbs that take "que", not "de que": "pienso de que" -> "pienso que".
const NO_DE_QUE: &[&str] = &[
    "pensar", "creer", "opinar", "decir", "considerar", "suponer", "imaginar", "afirmar", "comentar", "sostener", "señalar", "indicar", "comunicar", "anunciar", "manifestar", "declarar", "explicar", "demostrar", "desear",
    "esperar", "temer", "parecer", "resultar", "recomendar", "sugerir", "admitir", "reconocer", "negar", "asegurar", "intuir", "comprobar", "deducir", "concluir", "aceptar",
];

const DECIR: &[&str] = &["dije", "dijiste", "dijo", "dijimos", "dijisteis", "dijeron", "digo", "dices", "dice", "decimos", "decís", "dicen", "diré", "dirá", "dirán", "diría", "dirían", "decía", "decían"];

fn dequeismo(cx: &mut Ctx, i: usize) {
    let Some(j) = cx.next_word(i) else { return };
    if !cx.toks[j].is("que") {
        return;
    }
    let Some(v) = cx.prev_word(i) else { return };
    let verb = cx.toks[v].lower();
    // "me alegro de que", "se dio cuenta de que": pronominal verbs take it.
    if cx.prev_word(v).is_some_and(|p| matches!(cx.toks[p].lower().as_str(), "me" | "te" | "se" | "nos" | "os")) {
        return;
    }
    let hit = DECIR.contains(&verb.as_str())
        || cx.dict.is_some_and(|d| d.analyze(&verb).iter().any(|a| NO_DE_QUE.contains(&a.root.as_str()) && (a.last_affix().is_some_and(|x| VERB.contains(&x)) || a.affixes.is_empty())))
        // Without a dictionary: the commonest forms.
        || matches!(verb.as_str(), "pienso" | "piensa" | "pensaba" | "pensé" | "pensó" | "creo" | "cree" | "creía" | "creí" | "creyó" | "opino" | "opina" | "parece" | "pareció");
    // "pensar" itself is a noun-like infinitive only with an article; the
    // infinitive is still wrong with "de que".
    if hit {
        let r = cx.span(i, j);
        let to = case_like("que", cx.toks[i].s);
        cx.push(r, "es_dequeismo", vec![to]);
    }
}

/// "las casa", "el mesa", "un problemas".
fn agreement(cx: &mut Ctx, i: usize) {
    let Some(dict) = cx.dict else { return };
    let Some(j) = cx.next_word(i) else { return };
    let d = cx.toks[i].lower();
    let word = cx.toks[j].s;
    // Names and words in capitals.
    if word.chars().any(char::is_uppercase) || word.chars().count() < 2 || NUMERALS.contains(&word) {
        return;
    }
    // "del" and "al" are in DETS with two words; here only the one-word ones.
    let Some((row, dg, dn)) = det(&d) else { return };
    let all = dict.analyze(word);
    let Some(mut nom) = nominal(word, &all) else { return };
    if INVARIABLE.contains(&word) {
        nom.n = N::Both;
    }
    // "Juan las casa": a pronoun before a verb, unless after a preposition.
    if (nom.verb || FINITE.contains(&word)) && CLITIC.contains(&d.as_str()) && !cx.prev_word(i).is_some_and(|p| PREPOSITIONS.contains(&cx.toks[p].lower().as_str())) {
        return;
    }
    let lower = word.to_lowercase();
    let stressed = STRESSED_A.contains(&lower.as_str()) || nom.singular.as_deref().is_some_and(|s| STRESSED_A.contains(&s));
    let number_bad = matches!((dn, nom.n), (N::Sing, N::Plur) | (N::Plur, N::Sing));
    let gender_bad = match (dg, nom.g) {
        (Some(a), Some(b)) if a != b => {
            // "el agua", "un hacha": masculine-looking before a stressed a.
            !(stressed && a == G::M && b == G::F && dn == N::Sing && matches!(DETS[row][0], "el" | "un" | "algún" | "ningún"))
        }
        // "la agua" -> "el agua".
        (Some(G::F), Some(G::F)) => stressed && dn == N::Sing && nom.n != N::Plur && matches!(DETS[row][0], "el" | "un" | "algún" | "ningún"),
        _ => false,
    };
    if !number_bad && !gender_bad {
        return;
    }
    let r = cx.span(i, j);
    let ds = cx.toks[i].s;
    let mut sugg = Vec::new();
    // The noun's form for the determiner (the number only).
    if number_bad && !gender_bad {
        let form = match dn {
            N::Plur => Some(plural(word)),
            _ => nom.singular.clone(),
        };
        if let Some(f) = form.filter(|f| dict.check(f)) {
            sugg.push(format!("{ds} {f}"));
        }
    }
    // The determiner for the noun.
    let g = nom.g.or(dg).unwrap_or(G::M);
    let n = if nom.n == N::Both { dn } else { nom.n };
    let noun_sing = nom.singular.as_deref().unwrap_or(&lower).to_string();
    let to = det_for(row, g, n, &noun_sing);
    if to != d {
        sugg.push(format!("{} {word}", case_like(to, ds)));
    }
    if !sugg.is_empty() {
        cx.push(r, "es_agreement", sugg);
    }
}

/// Most of the words aren't Spanish: a quote in another language.
fn foreign(dict: Option<&crate::Dictionary>, text: &str) -> bool {
    let Some(d) = dict else { return false };
    let words: Vec<&str> = crate::words(text).map(|w| w.1).collect();
    words.iter().filter(|w| !d.check(w)).count() * 2 > words.len()
}

/// "Qué hora es?" -> "¿Qué hora es?"; "¿Vienes." -> "¿Vienes?".
fn marks(cx: &mut Ctx) {
    let text = cx.text;
    let mut out = Vec::new();
    for (pos, c) in text.char_indices() {
        let (open, close) = match c {
            '?' => ('¿', '?'),
            '!' => ('¡', '!'),
            '¿' => ('¿', '?'),
            '¡' => ('¡', '!'),
            _ => continue,
        };
        if c == close {
            // "?!" and "!!": one closing.
            if text[..pos].ends_with(['?', '!']) {
                continue;
            }
            // Back to the sentence's start.
            let before = &text[..pos];
            // Not a mark: "Hoja!A1", "#CONNECT!", "Usa ? para".
            let next = text[pos + 1..].chars().next();
            if next.is_some_and(|x| !x.is_whitespace() && !matches!(x, '»' | '"' | '”' | ')' | ',' | '.' | ';' | ':' | '—'))
                || before.ends_with(|x: char| x.is_whitespace() || matches!(x, '«' | '"' | '“' | '(' | '[' | '`' | '\'')) || before.rsplit(char::is_whitespace).next().is_some_and(|w| w.contains('#')) {
                continue;
            }
            let start = before.rfind(|x: char| matches!(x, '.' | '?' | '!' | '…' | ':' | ';' | '\n' | '\u{2028}' | '¿' | '¡')).map_or(0, |k| k + before[k..].chars().next().map_or(1, char::len_utf8));
            if start > 0 && before[..start].ends_with(['¿', '¡']) {
                continue;
            }
            let sentence = &text[start..pos];
            let trimmed = sentence.trim_start_matches(|x: char| x.is_whitespace() || matches!(x, '—' | '-' | '–'));
            let s0 = start + (sentence.len() - trimmed.len());
            // A lone mark, or a word in another script.
            if !trimmed.chars().any(char::is_alphabetic) {
                continue;
            }
            if foreign(cx.dict, trimmed) {
                continue;
            }
            let end = pos + c.len_utf8();
            let fixed = format!("{open}{}", &text[s0..end]);
            out.push((s0..end, vec![fixed]));
        } else {
            // An opening mark: its closing one before the sentence ends.
            let rest = &text[pos + c.len_utf8()..];
            if !rest.starts_with(|x: char| x.is_alphanumeric() || matches!(x, '«' | '"' | '“')) {
                continue;
            }
            let stop = rest.find(|x: char| x == close || matches!(x, '.' | '¿' | '¡' | '\n' | '\u{2028}') || (x != close && matches!(x, '?' | '!')));
            match stop.map(|k| (k, rest[k..].chars().next().unwrap_or(' '))) {
                Some((_, x)) if x == close || matches!(x, '?' | '!') => {}
                found => {
                    let end_in_rest = match found {
                        Some((k, _)) => k,
                        None => rest.trim_end().len(),
                    };
                    let body = rest[..end_in_rest].trim_end();
                    if body.is_empty() {
                        continue;
                    }
                    let a = pos;
                    // The point is replaced by the closing mark.
                    let has_point = matches!(found, Some((k, '.')) if rest[..k].trim_end().len() == body.len() && k == body.len());
                    let b = pos + c.len_utf8() + body.len() + usize::from(has_point);
                    out.push((a..b, vec![format!("{open}{body}{close}")]));
                }
            }
        }
    }
    for (r, s) in out {
        cx.push(r, "es_marks", s);
    }
}

#[cfg(test)]
mod tests {
    use super::super::check_grammar;

    fn rules(text: &str) -> Vec<(&'static str, String, Vec<String>)> {
        check_grammar(text, "es_ES", None).into_iter().map(|i| (i.rule, text[i.range].to_string(), i.suggestions)).collect()
    }

    fn one(text: &str) -> (&'static str, String, Vec<String>) {
        let r = rules(text);
        assert_eq!(r.len(), 1, "{text}: {r:?}");
        r.into_iter().next().unwrap()
    }

    #[test]
    fn contractions_and_a_ver() {
        assert_eq!(one("Fui a el cine"), ("es_contraction", "a el".into(), vec!["al".into()]));
        assert_eq!(one("De el todo"), ("es_contraction", "De el".into(), vec!["Del".into()]));
        assert!(rules("Vengo de El Salvador y de él.").is_empty());
        assert_eq!(one("Haber si vienes"), ("es_a_ver", "Haber".into(), vec!["A ver".into()]));
        assert_eq!(one("Debe a ver llegado"), ("es_a_ver", "a ver".into(), vec!["haber".into()]));
        assert!(rules("Vamos a ver la casa. Puede haber gente.").is_empty());
    }

    #[test]
    fn sino_porque_and_de_que() {
        assert_eq!(one("No sé si no que hacer"), ("es_sino", "si no".into(), vec!["sino".into()]));
        assert_eq!(one("Llámame sino puedes"), ("es_sino", "sino".into(), vec!["si no".into()]));
        assert_eq!(one("Llámame sino lo tienes"), ("es_sino", "sino".into(), vec!["si no".into()]));
        assert!(rules("No es rojo sino azul.").is_empty());
        assert_eq!(one("¿Porque no vienes?"), ("es_porque", "Porque".into(), vec!["Por qué".into()]));
        assert_eq!(one("No sé el porque."), ("es_porque", "porque".into(), vec!["porqué".into()]));
        assert_eq!(one("Vino porqué quiso."), ("es_porque", "porqué".into(), vec!["porque".into(), "por qué".into()]));
        assert!(rules("Vino porque quiso. No sé el porqué. ¿Por qué?").is_empty());
        assert_eq!(one("Pienso de que sí"), ("es_dequeismo", "de que".into(), vec!["que".into()]));
        assert!(rules("Me alegro de que vengas. Estoy seguro de que sí.").is_empty());
    }

    #[test]
    fn question_and_exclamation_marks() {
        assert_eq!(one("Qué hora es?"), ("es_marks", "Qué hora es?".into(), vec!["¿Qué hora es?".into()]));
        assert_eq!(one("Hola. Vienes!"), ("es_marks", "Vienes!".into(), vec!["¡Vienes!".into()]));
        assert_eq!(one("¿Vienes. Sí."), ("es_marks", "¿Vienes.".into(), vec!["¿Vienes?".into()]));
        assert_eq!(one("Dime, ¿vienes"), ("es_marks", "¿vienes".into(), vec!["¿vienes?".into()]));
        assert!(rules("Si llueve, ¿vienes? ¡Claro! ¿¡Qué!? Dijo: «¿Sí?»").is_empty());
    }
}
