//! Small inline dictionaries exercising one Hunspell feature each. The
//! expected results were cross-checked against libhunspell 1.7.

use apex_spell::Dictionary;

fn dict(aff: &str, dic: &str) -> Dictionary {
    Dictionary::from_strings(aff, dic).expect("fixture parses")
}

#[track_caller]
fn expect(d: &Dictionary, good: &[&str], wrong: &[&str]) {
    for w in good {
        assert!(d.check(w), "{w:?} should be correct");
    }
    for w in wrong {
        assert!(!d.check(w), "{w:?} should be wrong");
    }
}

#[test]
fn affixes_cross_product_and_twofold() {
    let d = dict(
        "SET UTF-8
TRY esianrtolcdugmphbyfvkwz
PFX U Y 1
PFX U 0 un .
SFX S Y 3
SFX S y ies [^aeiou]y
SFX S 0 s [aeiou]y
SFX S 0 s [^y]
SFX P Y 2
SFX P y iness [^aeiou]y
SFX P 0 ness [^y]
SFX R N 1
SFX R 0 er .
PFX I N 1
PFX I 0 in .
SFX C Y 1
SFX C r ción/T ar
SFX T Y 2
SFX T 0 s [aeiou]
SFX T ón ones ón
",
        "7
happy/UP
boy/S
fly/S
kind/URI
comprar/C
canción/T
house/S
",
    );
    expect(
        &d,
        &[
            "happy", "unhappy", "unhappiness", "happiness", "boys", "flies", "kind", "unkind", "kinder", "inkind",
            "compración", "compraciones", "canciones", "Happy", "HAPPY", "UNHAPPINESS", "Houses",
        ],
        &[
            "happyness", "unhappyness", "boies", "flys", "unboy", "unkinder", "inkinder", "comprarción", "compracións",
            "canciónes", "unkindness", "houss", "hAPPY", "unhouse",
        ],
    );
}

#[test]
fn long_flags_and_special_flags() {
    let d = dict(
        "FLAG long
NEEDAFFIX Na
FORBIDDENWORD Fb
KEEPCASE Kc
NOSUGGEST Ns
ONLYINCOMPOUND Oc
SFX Sa Y 1
SFX Sa 0 s .
SFX Sb Y 1
SFX Sb 0 ed .
",
        "8
pseudo/NaSa
bar/SaSb
bars/Fb
iPod/KcSa
OpenOffice.org/Kc
darn/NsSa
foo/SaSb
xyz/Oc
",
    );
    expect(
        &d,
        &["pseudos", "bar", "bared", "iPod", "iPods", "OpenOffice.org", "darn", "darns", "foos", "fooed"],
        &["pseudo", "bars", "IPOD", "Ipod", "ipod", "OPENOFFICE.ORG", "Openoffice.org", "xyz", "Pseudo", "BARS"],
    );
    // NOSUGGEST words are correct but never suggested.
    assert!(!d.suggest("darm", 5).iter().any(|s| s == "darn"));
    assert!(d.suggest("fooo", 5).contains(&"foo".to_string()));
}

#[test]
fn numeric_flags() {
    let d = dict(
        "FLAG num
SFX 101 Y 1
SFX 101 0 s .
PFX 2002 Y 1
PFX 2002 0 re .
",
        "2
load/101,2002
do/2002
",
    );
    expect(&d, &["load", "loads", "reload", "reloads", "redo"], &["dos", "redos", "reloadss"]);
}

#[test]
fn flag_aliases() {
    let d = dict(
        "AF 2
AF AB
AF A
SFX A Y 1
SFX A 0 s .
PFX B Y 1
PFX B 0 re .
SFX C Y 1
SFX C 0 ing/1 .
",
        "3
make/1
take/2
work
",
    );
    expect(&d, &["make", "makes", "remake", "remakes", "take", "takes", "work"], &["retake", "works", "reworks"]);
}

#[test]
fn compounding_with_flags() {
    let d = dict(
        "COMPOUNDFLAG X
COMPOUNDMIN 3
ONLYINCOMPOUND O
COMPOUNDBEGIN B
COMPOUNDMIDDLE M
COMPOUNDEND E
SFX S Y 1
SFX S 0 s .
",
        "8
foot/XS
ball/XS
fu/X
hand/B
werk/M
zeug/E
schaft/OX
bar/X
",
    );
    expect(
        &d,
        &[
            "football", "footballs", "ballfoot", "footballfoot", "handzeug", "handwerkzeug", "handwerkwerkzeug",
            "footschaft", "barbar", "foot", "footbar", "handfoot", "zeug",
        ],
        &["fuball", "footfu", "zeughand", "werkzeug", "handwerk", "schaft"],
    );
}

#[test]
fn compound_rules_for_ordinals() {
    let d = dict(
        "COMPOUNDMIN 1
ONLYINCOMPOUND c
COMPOUNDRULE 2
COMPOUNDRULE n*1t
COMPOUNDRULE n*mp
",
        "8
0/nm
1/n1
1st/p
1th/tc
2/nm
2nd/p
2th/tc
3th/tc
",
    );
    expect(
        &d,
        &["1st", "2nd", "11th", "21st", "22nd", "121st", "12th"],
        &["1th", "2th", "21th", "12nd", "11st", "0th"],
    );
}

#[test]
fn circumfix() {
    let d = dict(
        "CIRCUMFIX X
PFX A Y 1
PFX A 0 leg/X .
PFX B Y 1
PFX B 0 legesleg/X .
SFX C Y 3
SFX C 0 obb .
SFX C 0 obb/AX .
SFX C 0 obb/BX .
",
        "1
nagy/C
",
    );
    expect(
        &d,
        &["nagy", "nagyobb", "legnagyobb", "legeslegnagyobb"],
        &["legnagy", "legeslegnagy", "legesleg", "nagyobbobb"],
    );
}

#[test]
fn input_output_conversion_and_ignore() {
    let d = dict(
        "SET UTF-8
ICONV 2
ICONV ’ '
ICONV ﬁ fi
OCONV 1
OCONV ' ’
IGNORE ˇ
WORDCHARS '
",
        "4
don't
file
ab
o'clock
",
    );
    expect(&d, &["don't", "don’t", "ﬁle", "aˇb", "o’clock", "abˇ"], &["dont", "fille"]);
    assert_eq!(d.word_chars(), "'");
    // OCONV applies to suggestions.
    assert_eq!(d.suggest("dont", 3).first().map(String::as_str), Some("don’t"));
}

#[test]
fn break_points() {
    let d = dict(
        "BREAK 3
BREAK -
BREAK ^-
BREAK -$
",
        "4
well
known
e-mail
foo
",
    );
    expect(
        &d,
        &["well-known", "e-mail", "well-known-foo", "-foo", "foo-", "e-mail-foo"],
        &["well-knwn", "wel-known", "e-mial"],
    );
    // Hyphenated words get the wrong part fixed.
    assert!(d.suggest("well-knwn", 5).contains(&"well-known".to_string()));

    // Without the default patterns, hyphenated words must be listed.
    let d = dict("BREAK 0\n", "2\nwell\nknown\n");
    expect(&d, &["well", "known"], &["well-known"]);
    // No BREAK directive at all means the defaults.
    let d = dict("", "2\nwell\nknown\n");
    expect(&d, &["well-known"], &[]);
}

#[test]
fn capitalization() {
    let d = dict(
        "KEEPCASE K
SFX S Y 1
SFX S 0 s .
",
        "6
Paris/S
NASA
hello/S
McDonald
iPhone/K
LaTeX/K
",
    );
    expect(
        &d,
        &[
            "Paris", "PARIS", "NASA", "hello", "Hello", "HELLO", "hellos", "HELLOS", "McDonald", "MCDONALD", "iPhone",
            "LaTeX", "Pariss", "PARISS",
        ],
        &["paris", "nasa", "Nasa", "hELLO", "Mcdonald", "mcdonald", "IPHONE", "Iphone", "LATEX", "latex", "Paris's"],
    );
}

#[test]
fn numbers_abbreviations_and_trivia() {
    let d = dict("", "2\netc.\nword\n");
    expect(
        &d,
        &["1990", "3.5", "1,000", "10-20", "etc.", "word", "word.", "", "  word  ", "...", "word\u{AD}"],
        &["etc", "wrd", "1990s", "wo rd"],
    );
    // Absurdly long input is rejected rather than processed.
    assert!(!d.check(&"a".repeat(1000)));
}

#[test]
fn suggestion_rules() {
    let d = dict(
        "SET UTF-8
TRY aeiouáéíóústnrlcdphmbgfvyzjkqwx
KEY qwertyuiop|asdfghjkl|zxcvbnm
REP 2
REP f ph
REP ^nu new
MAP 2
MAP aá
MAP eéè
NOSUGGEST !
",
        "9
phone
café
test
new
the
receive
lot
a
damn/!
",
    );
    let s = |w: &str| d.suggest(w, 5);
    assert_eq!(s("fone"), ["phone"]); // REP
    assert_eq!(s("cafe"), ["café"]); // MAP
    assert_eq!(s("tesr"), ["test"]); // KEY neighbour
    assert_eq!(s("teh"), ["the"]); // swap
    assert_eq!(s("recieve"), ["receive"]);
    assert_eq!(s("alot"), ["lot", "a lot"]); // delete, split
    assert_eq!(s("nu"), ["new"]); // anchored REP
    assert_eq!(s("Fone"), ["Phone"]); // case kept
    assert_eq!(s("CAFE"), ["CAFÉ"]);
    assert!(s("dam").is_empty()); // NOSUGGEST
    assert_eq!(s("tst"), ["test"]); // insert
    assert!(d.suggest("fone", 0).is_empty());
    assert!(s("").is_empty());
}

#[test]
fn latin1_files_load() {
    let dir = std::env::temp_dir().join(format!("apex-spell-latin1-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let aff = dir.join("xx_XX.aff");
    let dic = dir.join("xx_XX.dic");
    // "SET ISO8859-1" with a Latin-1 flag (0xE9) and word "café".
    std::fs::write(&aff, b"SET ISO8859-1\nSFX \xE9 Y 1\nSFX \xE9 0 s .\n").expect("write");
    std::fs::write(&dic, b"1\ncaf\xE9/\xE9\n").expect("write");
    let d = Dictionary::load(&aff, &dic).expect("load");
    assert_eq!(d.language(), "xx_XX");
    assert!(d.check("café") && d.check("cafés") && d.check("CAFÉS"));
    assert!(!d.check("cafe"));
    assert_eq!(d.suggest("cafe", 3), ["café"]);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(Dictionary::load("/nonexistent/x.aff".as_ref(), "/nonexistent/x.dic".as_ref()).is_err());
}

#[test]
fn garbage_input_does_not_panic() {
    // Malformed affix lines, unbalanced conditions, bad flags, odd text.
    let d = dict(
        "SET UTF-8
FLAG long
PFX
SFX Ab Y 2
SFX Ab 0 s [abc
SFX Ab
PFX Cd Y x
REP 1
REP
MAP 1
MAP (ab
COMPOUNDRULE 1
COMPOUNDRULE (A)*(
AF 1
AF
",
        "3\nfoo/AbZZZ\n/\n\\/bar/Ab\n\n",
    );
    for w in ["foo", "foos", "", "\u{0}", "a-", "-", "--", "'", "a'b'c", "ǅemal", "ß", "ΣΊΣΥΦΟΣ", "🙂", "a\u{301}"] {
        let _ = d.check(w);
        let _ = d.suggest(w, 5);
    }
    let d = dict("", "");
    assert!(!d.check("anything"));
    assert!(d.suggest("anything", 5).is_empty());
}
