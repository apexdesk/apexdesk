//! A small argument parser for the subcommands. The command line needs
//! positional arguments, `--flag`, `--opt value`, `--opt=value`, short
//! aliases and `--`; a hundred lines do that without a dependency, and
//! keep negative numbers and formulas ("-5", "=-A1") as values.

/// Options a subcommand takes.
pub struct Spec {
    /// Options with a value, long names without dashes.
    pub values: &'static [&'static str],
    /// Options without a value.
    pub flags: &'static [&'static str],
}

/// Short aliases shared by every subcommand.
const SHORT: &[(char, &str)] = &[('o', "output"), ('d', "out"), ('t', "to"), ('s', "sheet"), ('r', "range"), ('f', "format"), ('q', "quiet"), ('h', "help"), ('p', "password"), ('w', "width")];

#[derive(Debug, Default)]
pub struct Parsed {
    pub pos: Vec<String>,
    opts: Vec<(&'static str, Option<String>)>,
}

impl Parsed {
    /// The last value given to an option.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.opts.iter().rev().find(|(n, _)| *n == name).and_then(|(_, v)| v.as_deref())
    }

    pub fn flag(&self, name: &str) -> bool {
        self.opts.iter().any(|(n, _)| *n == name)
    }
}

/// Parses `args` (after the subcommand); `--help` is always known.
pub fn parse(args: &[String], spec: &Spec) -> Result<Parsed, String> {
    let find = |name: &str| -> Option<(&'static str, bool)> {
        if name == "help" {
            return Some(("help", false));
        }
        if let Some(v) = spec.values.iter().find(|v| **v == name) {
            return Some((v, true));
        }
        spec.flags.iter().find(|f| **f == name).map(|f| (*f, false))
    };
    let mut out = Parsed::default();
    let mut it = args.iter();
    let mut rest = false;
    while let Some(a) = it.next() {
        let is_opt = !rest && a.len() > 1 && a.starts_with('-') && !a[1..].starts_with(|c: char| c.is_ascii_digit() || c == '.');
        if !is_opt {
            out.pos.push(a.clone());
            continue;
        }
        if a == "--" {
            rest = true;
            continue;
        }
        let (name, inline) = match a.strip_prefix("--") {
            Some(long) => match long.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (long.to_string(), None),
            },
            None => {
                // "-o file", "-ofile" is not supported: short options stand alone.
                let mut chars = a[1..].chars();
                let (Some(c), None) = (chars.next(), chars.next()) else { return Err(format!("unknown option '{a}'")) };
                match SHORT.iter().find(|(s, _)| *s == c) {
                    Some((_, long)) => (long.to_string(), None),
                    None => return Err(format!("unknown option '{a}'")),
                }
            }
        };
        // A document's password from the environment (not on the command
        // line, where other users see it).
        if name == "password-env" && spec.values.contains(&"password") && !spec.values.contains(&"password-env") {
            let var = match inline {
                Some(v) => v,
                None => it.next().cloned().ok_or_else(|| "option '--password-env' needs a value".to_string())?,
            };
            let pw = std::env::var(&var).map_err(|_| format!("environment variable '{var}' is not set"))?;
            out.opts.push(("password", Some(pw)));
            continue;
        }
        let Some((name, takes)) = find(&name) else { return Err(format!("unknown option '{a}'")) };
        let value = match (takes, inline) {
            (true, Some(v)) => Some(v),
            (true, None) => Some(it.next().cloned().ok_or_else(|| format!("option '--{name}' needs a value"))?),
            (false, Some(_)) => return Err(format!("option '--{name}' takes no value")),
            (false, None) => None,
        };
        out.opts.push((name, value));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_values() {
        let spec = Spec { values: &["to", "sheet"], flags: &["quiet"] };
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let p = parse(&a(&["a.xlsx", "--to", "pdf", "-q", "-s=2", "-5", "--", "--x"]), &spec);
        // "-s=2" is not a short option.
        assert!(p.is_err());
        let p = parse(&a(&["a.xlsx", "--to=pdf", "-q", "-s", "2", "-5", "--", "--x"]), &spec).unwrap();
        assert_eq!(p.pos, ["a.xlsx", "-5", "--x"]);
        assert_eq!((p.value("to"), p.value("sheet"), p.flag("quiet")), (Some("pdf"), Some("2"), true));
        assert!(parse(&a(&["--nope"]), &spec).unwrap_err().contains("--nope"));
        assert!(parse(&a(&["--to"]), &spec).is_err());
        assert!(parse(&a(&["--quiet=1"]), &spec).is_err());
        // Aliases of options the command doesn't take are unknown.
        assert!(parse(&a(&["-r", "A1"]), &spec).is_err());
    }
}
