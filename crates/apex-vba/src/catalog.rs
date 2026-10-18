//! What the macro editor knows of VBA and of the object model: classes and
//! their members, functions, constants and keywords as they are written,
//! with their parameters and a line of help (`catalog.txt`). The Object
//! Browser, completion, parameter info, case and F1 help come from here.

use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Read and write.
    Property,
    ReadOnly,
    Method,
    Event,
    Function,
    Constant,
    Keyword,
}

#[derive(Clone, Debug)]
pub struct Member {
    pub name: &'static str,
    pub kind: Kind,
    /// `(Prompt, [Buttons], [Title])`: optional ones in brackets; empty
    /// for properties and constants.
    pub params: &'static str,
    /// The class or type it gives (`Range`, `String`...), or the value of
    /// a constant.
    pub ty: &'static str,
    pub help: &'static str,
}

#[derive(Clone, Debug)]
pub struct Class {
    pub name: &'static str,
    pub help: &'static str,
    pub members: Vec<Member>,
}

struct Catalog {
    classes: Vec<Class>,
    /// Lower-case name → as written, of everything.
    case: HashMap<String, &'static str>,
}

fn catalog() -> &'static Catalog {
    static C: OnceLock<Catalog> = OnceLock::new();
    C.get_or_init(|| parse(include_str!("catalog.txt")))
}

/// Word's object model (`catalog_word.txt`); the language's classes come
/// from the main catalog.
fn word_catalog() -> &'static Catalog {
    static C: OnceLock<Catalog> = OnceLock::new();
    C.get_or_init(|| parse(include_str!("catalog_word.txt")))
}

/// Classes of the main catalog that aren't Excel's: Word macros have them too.
const SHARED: &[&str] = &["Keywords", "VBA", "Constants", "FileDialog", "FileDialogFilters", "FileDialogFilter", "FileDialogSelectedItems", "Collection", "Dictionary", "FileSystemObject", "TextStream", "File", "Folder", "Files", "Folders", "ErrObject", "Debug", "Class"];

fn parse(text: &'static str) -> Catalog {
    {
        let mut classes: Vec<Class> = Vec::new();
        for line in text.lines() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (head, help) = line.split_once(" | ").unwrap_or((line, ""));
            if let Some(name) = head.strip_prefix("class ") {
                classes.push(Class { name, help, members: Vec::new() });
                continue;
            }
            let (tag, rest) = head.split_at(1);
            let rest = rest.trim();
            let kind = match tag {
                "p" => Kind::Property,
                "r" => Kind::ReadOnly,
                "m" => Kind::Method,
                "e" => Kind::Event,
                "f" => Kind::Function,
                "c" => Kind::Constant,
                _ => Kind::Keyword,
            };
            let (sig, ty) = match kind {
                Kind::Constant => rest.split_once(" = ").unwrap_or((rest, "")),
                // The type follows the last `): ` or the name's `: `.
                _ => match rest.rfind("): ").map(|i| i + 1).or_else(|| (!rest.contains('(')).then(|| rest.find(": ")).flatten()) {
                    Some(i) => (&rest[..i], rest[i + 1..].trim_start_matches(':').trim()),
                    None => (rest, ""),
                },
            };
            let (name, params) = match sig.find('(') {
                Some(i) => (&sig[..i], &sig[i..]),
                None => (sig, ""),
            };
            if let Some(c) = classes.last_mut() {
                c.members.push(Member { name, kind, params, ty, help });
            }
        }
        let mut case = HashMap::new();
        for c in &classes {
            case.entry(c.name.to_ascii_lowercase()).or_insert(c.name);
            for m in &c.members {
                case.entry(m.name.to_ascii_lowercase()).or_insert(m.name);
            }
        }
        // Types and names the code often writes.
        for w in ["Scripting", "Variant", "Object", "VbMsgBoxResult", "LBound", "UBound", "Option", "Compare", "Text", "Binary", "Base", "Rem", "Then", "Else", "On", "Error", "Resume", "Next"] {
            case.entry(w.to_ascii_lowercase()).or_insert(w);
        }
        Catalog { classes, case }
    }
}

/// Every class (Keywords, VBA, Global, Application, Range...).
pub fn classes() -> &'static [Class] {
    &catalog().classes
}

/// The classes a macro of Word (`word`) or of a workbook knows: Word's own
/// object model and the language's.
pub fn classes_of(word: bool) -> Vec<&'static Class> {
    if !word {
        return classes().iter().collect();
    }
    let shared = classes().iter().filter(|c| SHARED.contains(&c.name));
    word_catalog().classes.iter().chain(shared).collect()
}

/// A class by name for a macro of Word or of a workbook.
pub fn class_of(word: bool, name: &str) -> Option<&'static Class> {
    if !word {
        return class(name);
    }
    let low = name.to_ascii_lowercase();
    word_catalog().classes.iter().find(|c| c.name.eq_ignore_ascii_case(&low)).or_else(|| class(name).filter(|c| SHARED.contains(&c.name)))
}

/// A member of a class for a macro of Word or of a workbook.
pub fn member_of(word: bool, class_name: &str, name: &str) -> Option<&'static Member> {
    class_of(word, class_name)?.members.iter().find(|m| m.name.eq_ignore_ascii_case(name))
}

/// A name with no object before it, for a macro of Word or of a workbook.
pub fn global_of(word: bool, name: &str) -> Option<(&'static Class, &'static Member)> {
    if !word {
        return global(name);
    }
    ["VBA", "Global", "Constants", "WordConstants", "Keywords"].iter().find_map(|c| {
        let c = class_of(true, c)?;
        Some((c, c.members.iter().find(|m| m.name.eq_ignore_ascii_case(name))?))
    })
}

/// A class by name (case ignored); `Sheets` is `Worksheets`.
pub fn class(name: &str) -> Option<&'static Class> {
    let name = match name.to_ascii_lowercase().as_str() {
        "sheets" => "worksheets".to_string(),
        "scripting.dictionary" => "dictionary".to_string(),
        "scripting.filesystemobject" => "filesystemobject".to_string(),
        "err" => "errobject".to_string(),
        n => n.to_string(),
    };
    classes().iter().find(|c| c.name.eq_ignore_ascii_case(&name))
}

/// A member of a class (case ignored).
pub fn member(class_name: &str, name: &str) -> Option<&'static Member> {
    class(class_name)?.members.iter().find(|m| m.name.eq_ignore_ascii_case(name))
}

/// A name with no object before it: a function, an Excel global, a
/// constant or a keyword.
pub fn global(name: &str) -> Option<(&'static Class, &'static Member)> {
    ["VBA", "Global", "Constants", "Keywords"].iter().find_map(|c| {
        let c = class(c)?;
        Some((c, c.members.iter().find(|m| m.name.eq_ignore_ascii_case(name))?))
    })
}

/// How VBA writes a known word (`msgbox` → `MsgBox`).
pub fn case_of(word: &str) -> Option<&'static str> {
    let low = word.to_ascii_lowercase();
    catalog().case.get(&low).or_else(|| word_catalog().case.get(&low)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_whole() {
        let m = member("Range", "offset").unwrap();
        assert_eq!((m.name, m.kind, m.params, m.ty), ("Offset", Kind::Method, "([RowOffset], [ColumnOffset])", "Range"));
        let v = member("range", "Value").unwrap();
        assert_eq!((v.kind, v.params, v.ty), (Kind::Property, "", "Variant"));
        let f = global("MSGBOX").unwrap().1;
        assert_eq!((f.params, f.ty), ("(Prompt, [Buttons], [Title])", "VbMsgBoxResult"));
        assert_eq!(global("randomize").unwrap().1.ty, "");
        assert_eq!(member("Sheets", "add").unwrap().ty, "Worksheet");
        assert_eq!(member("Workbook", "BeforeSave").unwrap().params, "(ByVal SaveAsUI As Boolean, Cancel As Boolean)");
        assert_eq!(case_of("vbcrlf"), Some("vbCrLf"));
        assert_eq!(case_of("elseif"), Some("ElseIf"));
        assert!(classes().iter().all(|c| !c.members.is_empty()));
    }

    #[test]
    fn constants_are_the_interpreters() {
        let c = class("Constants").unwrap();
        for m in &c.members {
            assert!(crate::consts::constant(&m.name.to_ascii_lowercase()).is_some(), "{} isn't a constant", m.name);
        }
        // And every constant of the interpreter is listed.
        let src = include_str!("consts.rs");
        for name in src.split('"').skip(1).step_by(2).filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric()) && (s.starts_with("vb") || s.starts_with("xl"))) {
            assert!(c.members.iter().any(|m| m.name.eq_ignore_ascii_case(name)), "{name} isn't in the catalog");
        }
    }

    #[test]
    fn words_own() {
        assert_eq!(member_of(true, "Range", "paragraphs").unwrap().ty, "Paragraphs");
        assert_eq!(member_of(true, "Document", "Content").unwrap().ty, "Range");
        assert_eq!(member_of(true, "Documents", "Open").unwrap().ty, "Document");
        // Excel's Range isn't Word's; the language's classes are shared.
        assert!(member_of(true, "Range", "Offset").is_none());
        assert!(member_of(false, "Range", "Offset").is_some());
        assert!(class_of(true, "Worksheet").is_none());
        assert!(class_of(true, "Dictionary").is_some());
        assert_eq!(global_of(true, "activedocument").unwrap().1.ty, "Document");
        assert!(global_of(false, "activedocument").is_none());
        assert_eq!(case_of("wdcollapseend"), Some("wdCollapseEnd"));
        assert!(classes_of(true).iter().all(|c| !c.members.is_empty()));
        for m in &class_of(true, "WordConstants").unwrap().members {
            assert!(crate::consts::constant(&m.name.to_ascii_lowercase()).is_some(), "{} isn't a constant", m.name);
        }
        // Every type a member gives that looks like a class is one.
        for c in &word_catalog().classes {
            for m in &c.members {
                let ty = m.ty;
                if !ty.is_empty() && ty.chars().next().is_some_and(char::is_uppercase) && !matches!(ty, "String" | "Long" | "Boolean" | "Double" | "Single" | "Variant" | "Date" | "Integer" | "Object" | "Byte" | "Currency") && m.kind != Kind::Constant {
                    assert!(class_of(true, ty).is_some(), "{}.{}: {ty}", c.name, m.name);
                }
            }
        }
    }

    #[test]
    fn functions_run() {
        // Every function listed is one the interpreter knows.
        let src = include_str!("builtins.rs");
        for m in &class("VBA").unwrap().members {
            let low = m.name.to_ascii_lowercase();
            assert!(src.contains(&format!("\"{low}\"")), "{} isn't a function of the interpreter", m.name);
        }
    }
}
