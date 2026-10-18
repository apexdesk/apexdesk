//! English rules.

use super::{Ctx, Kind, case_like};

/// Words that start with a vowel letter but a consonant sound.
const YOU_SOUND: &[&str] = &["uni", "use", "usu", "uti", "ura", "ure", "uro", "eu", "ewe", "one", "once", "ubi", "uga", "uku", "ufo", "uvu"];

/// Words that start with a silent "h".
const SILENT_H: &[&str] = &["hour", "honest", "honor", "honour", "heir", "herb", "http", "html", "hdmi", "hdr", "mp3", "mp4", "svg", "xml", "rgb"];

/// Letters whose names start with a vowel: "an F-test", "an XPath".
const VOWEL_LETTERS: &str = "AEFHILMNORSX";

/// Wants "an": the word starts with a vowel sound.
fn vowel_sound(w: &str) -> Option<bool> {
    let l = w.to_lowercase();
    let first = l.chars().next()?;
    if !first.is_alphabetic() {
        return None;
    }
    // Initials ("an FBI agent", "a NATO base") are read in many ways.
    if w.chars().count() > 1 && w.chars().all(|c| c.is_uppercase()) {
        return None;
    }
    // Read as a letter: "F", "X-ray". ("XPath" or "SQLite" are said
    // both ways.)
    let mut cs = w.chars();
    let (c0, c1) = (cs.next()?, cs.next());
    if c0.is_uppercase() && c1.is_none_or(|c| c == '-') {
        return Some(VOWEL_LETTERS.contains(c0));
    }
    if c0.is_uppercase() && c1.is_some_and(char::is_uppercase) {
        return None;
    }
    if SILENT_H.iter().any(|h| l.starts_with(h)) {
        return Some(true);
    }
    if "aeiou".contains(first) {
        // "unit" sounds "you", "uninformed" doesn't.
        let you = YOU_SOUND.iter().any(|p| l.starts_with(p)) && !["unin", "unim", "unid"].iter().any(|p| l.starts_with(p));
        return Some(!you);
    }
    Some(false)
}

pub(super) fn check(cx: &mut Ctx) {
    for i in 0..cx.toks.len() {
        if cx.toks[i].kind != Kind::Word {
            continue;
        }
        let w = cx.toks[i].lower();
        match w.as_str() {
            "a" | "an" => a_an(cx, i, &w),
            "could" | "should" | "would" | "must" | "might" => of_have(cx, i),
            "then" => than(cx, i),
            "your" => youre(cx, i),
            "i" | "i'm" | "i've" | "i'll" | "i'd" | "i’m" | "i’ve" | "i’ll" | "i’d" if cx.toks[i].s.starts_with('i') => {
                // Not "i.e.", "(i)", "x_i", "{i}", nor "i, ii, iii".
                let before_ok = i == 0 || cx.toks[i - 1].kind == Kind::Space;
                let after_ok = cx.toks.get(i + 1).is_none_or(|t| t.kind == Kind::Space || matches!(t.s, "," | ";" | ":" | "!" | "?"));
                let numeral = cx.text[cx.toks[i].r.end..].trim_start_matches([',', ' ']).starts_with("ii");
                if before_ok && after_ok && !numeral {
                    let r = cx.toks[i].r.clone();
                    let to = format!("I{}", &cx.toks[i].s[1..]);
                    cx.push(r, "en_i", vec![to]);
                }
            }
            _ => {}
        }
    }
}

/// "a apple" -> "an apple", "an car" -> "a car".
fn a_an(cx: &mut Ctx, i: usize, w: &str) {
    let Some(j) = cx.next_word(i) else { return };
    // "(a) first", "#N/A".
    if i > 0 && cx.toks[i - 1].kind != Kind::Space {
        return;
    }
    let Some(vowel) = vowel_sound(cx.toks[j].s) else { return };
    let want = if vowel { "an" } else { "a" };
    if want != w {
        let r = cx.toks[i].r.clone();
        let to = case_like(want, cx.toks[i].s);
        cx.push(r, "en_a_an", vec![to]);
    }
}

/// "could of" -> "could have".
fn of_have(cx: &mut Ctx, i: usize) {
    let Some(j) = cx.next_word(i) else { return };
    if cx.toks[j].is("of") {
        let r = cx.toks[j].r.clone();
        cx.push(r, "en_of_have", vec!["have".into()]);
    }
}

/// "better then" -> "better than".
fn than(cx: &mut Ctx, i: usize) {
    let Some(p) = cx.prev_word(i) else { return };
    let prev = cx.toks[p].lower();
    let comparative = matches!(prev.as_str(), "more" | "less" | "better" | "worse" | "rather" | "other" | "fewer" | "greater" | "smaller" | "larger" | "higher" | "lower" | "bigger" | "older" | "younger" | "faster" | "slower" | "longer" | "shorter" | "easier" | "harder" | "cheaper");
    // "and then", "since then": a word of time before it is fine; only
    // a comparative right before "then".
    if comparative {
        let r = cx.toks[i].r.clone();
        cx.push(r, "en_than", vec![case_like("than", cx.toks[i].s)]);
    }
}

/// "your welcome" -> "you're welcome".
fn youre(cx: &mut Ctx, i: usize) {
    let Some(j) = cx.next_word(i) else { return };
    let next = cx.toks[j].lower();
    if matches!(next.as_str(), "welcome" | "right" | "wrong" | "sure" | "not" | "very" | "so" | "too" | "the" | "a" | "an") {
        // "your welcome speech", "your right hand": a word after says it's
        // the possessive.
        let after = cx.next_word(j).map(|k| cx.toks[k].lower());
        if matches!(next.as_str(), "welcome" | "right" | "wrong") && after.is_some() {
            return;
        }
        let r = cx.toks[i].r.clone();
        cx.push(r, "en_youre", vec![case_like("you're", cx.toks[i].s)]);
    }
}

#[cfg(test)]
mod tests {
    use super::super::check_grammar;

    fn rules(text: &str) -> Vec<(&'static str, String, Vec<String>)> {
        check_grammar(text, "en_US", None).into_iter().map(|i| (i.rule, text[i.range].to_string(), i.suggestions)).collect()
    }

    #[test]
    fn a_or_an() {
        assert_eq!(rules("It was a apple."), [("en_a_an", "a".into(), vec!["an".into()])]);
        assert_eq!(rules("An car and an hour."), [("en_a_an", "An".into(), vec!["A".into()])]);
        assert!(rules("A university, an umbrella, a one-off, an honest man, an FBI agent, a euro, an 8, an F-test, a SQLite file, a URL-encoded text, #N/A.").is_empty());
    }

    #[test]
    fn mixed_up_words() {
        assert_eq!(rules("I could of known."), [("en_of_have", "of".into(), vec!["have".into()])]);
        assert_eq!(rules("It is better then that."), [("en_than", "then".into(), vec!["than".into()])]);
        assert_eq!(rules("Thanks! your welcome."), [("en_youre", "your".into(), vec!["you're".into()])]);
        assert!(rules("Your welcome speech was fine, and then we left. Since then, nothing.").is_empty());
        assert_eq!(rules("Then i left."), [("en_i", "i".into(), vec!["I".into()])]);
        assert_eq!(rules("Yes, i'm here."), [("en_i", "i'm".into(), vec!["I'm".into()])]);
        assert!(rules("Use i.e. or (i) here, x_i, {i} and i, ii, iii.").is_empty());
    }
}
