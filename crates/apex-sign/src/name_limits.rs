//! Name constraints (RFC 5280 4.2.1.10): a CA kept to some names (one
//! company's domain, its directory branch) vouches for nobody else.
//! E-mail addresses, DNS names and directory names are checked; a
//! constraint of another kind can't be, and the CA then vouches for no one.

use crate::asn1::Tlv;

/// A name of a certificate, or a subtree of names in a constraint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenName {
    Email(String),
    Dns(String),
    /// A distinguished name, as its relative names' encodings.
    Dir(Vec<Vec<u8>>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameLimits {
    pub permitted: Vec<GenName>,
    pub excluded: Vec<GenName>,
}

const EMAIL_OID: &str = "1.2.840.113549.1.9.1";

/// The relative names of a distinguished name.
pub(crate) fn rdns(name: &Tlv) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut r = name.children();
    while let Some(rdn) = r.next() {
        out.push(rdn.raw.to_vec());
    }
    out
}

/// The e-mail addresses in a distinguished name (the old emailAddress
/// attribute).
pub(crate) fn dn_emails(name: &Tlv) -> Vec<GenName> {
    let mut out = Vec::new();
    let mut r = name.children();
    while let Some(rdn) = r.next() {
        let mut atvs = rdn.children();
        while let Some(atv) = atvs.next() {
            let mut f = atv.children();
            if let (Some(ty), Some(v)) = (f.next(), f.next())
                && ty.oid() == EMAIL_OID
            {
                out.push(GenName::Email(v.string()));
            }
        }
    }
    out
}

/// One GeneralName: Some(None) for a kind not checked here, None for one
/// that can't be read.
fn general_name(n: &Tlv) -> Option<Option<GenName>> {
    Some(Some(match n.tag {
        0x81 => GenName::Email(std::str::from_utf8(n.body).ok()?.to_string()),
        0x82 => GenName::Dns(std::str::from_utf8(n.body).ok()?.to_string()),
        0xA4 => GenName::Dir(rdns(&n.inner().ok()?)),
        _ => return Some(None),
    }))
}

/// The names in some GeneralNames (subjectAltName) that are checked here;
/// None when one can't be read.
pub(crate) fn general_names(names: &Tlv) -> Option<Vec<GenName>> {
    let mut out = Vec::new();
    let mut r = names.children();
    while let Some(n) = r.next() {
        out.extend(general_name(&n)?);
    }
    Some(out)
}

/// The NameConstraints extension's value; None for one this can't apply
/// (a subtree of another kind of name, a minimum or a maximum).
pub(crate) fn parse(v: &Tlv) -> Option<NameLimits> {
    let mut limits = NameLimits::default();
    let mut r = v.children();
    while let Some(part) = r.next() {
        let list = match part.tag {
            0xA0 => &mut limits.permitted,
            0xA1 => &mut limits.excluded,
            _ => return None,
        };
        let mut subtrees = part.children();
        while let Some(st) = subtrees.next() {
            let mut f = st.children();
            let base = f.next()?;
            if f.next().is_some() {
                return None;
            }
            list.push(general_name(&base)??);
        }
    }
    Some(limits)
}

impl NameLimits {
    /// Whether these limits allow every one of `names`.
    pub fn allow(&self, names: &[GenName]) -> bool {
        names.iter().all(|n| {
            let kind = std::mem::discriminant(n);
            let permitted = self.permitted.iter().filter(|p| std::mem::discriminant(*p) == kind).collect::<Vec<_>>();
            (permitted.is_empty() || permitted.iter().any(|p| within(n, p, false))) && !self.excluded.iter().any(|x| within(n, x, true))
        })
    }
}

/// Whether `name` is in the subtree `base`. For exclusions directory names
/// compare without case: a name spelled differently must not slip out.
fn within(name: &GenName, base: &GenName, loose: bool) -> bool {
    match (name, base) {
        (GenName::Email(n), GenName::Email(b)) => {
            let n = n.to_ascii_lowercase();
            let b = b.to_ascii_lowercase();
            let host = n.rsplit_once('@').map_or(n.as_str(), |(_, h)| h);
            if b.contains('@') {
                n == b
            } else if b.starts_with('.') {
                host.ends_with(&b)
            } else {
                host == b
            }
        }
        (GenName::Dns(n), GenName::Dns(b)) => {
            let n = n.trim_end_matches('.').to_ascii_lowercase();
            let b = b.trim_start_matches('.').trim_end_matches('.').to_ascii_lowercase();
            b.is_empty() || n == b || n.ends_with(&format!(".{b}"))
        }
        (GenName::Dir(n), GenName::Dir(b)) => {
            b.len() <= n.len() && n.iter().zip(b).all(|(x, y)| if loose { x.eq_ignore_ascii_case(y) } else { x == y })
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(parts: &[&[u8]]) -> GenName {
        GenName::Dir(parts.iter().map(|p| p.to_vec()).collect())
    }

    #[test]
    fn subtrees() {
        let l = NameLimits { permitted: vec![GenName::Dns("example.com".into()), GenName::Email(".example.com".into()), dir(&[b"C=ES", b"O=Acme"])], excluded: vec![GenName::Dns("bad.example.com".into())] };
        assert!(l.allow(&[GenName::Dns("www.example.com".into())]));
        assert!(l.allow(&[GenName::Dns("example.com".into())]));
        assert!(!l.allow(&[GenName::Dns("badexample.com".into())]));
        assert!(!l.allow(&[GenName::Dns("x.bad.example.com".into())]));
        assert!(l.allow(&[GenName::Email("ana@mail.example.com".into())]));
        assert!(!l.allow(&[GenName::Email("ana@example.com".into())]));
        assert!(!l.allow(&[GenName::Email("ana@evil.com".into())]));
        assert!(l.allow(&[dir(&[b"C=ES", b"O=Acme", b"CN=Ana"])]));
        assert!(!l.allow(&[dir(&[b"C=ES", b"O=Other", b"CN=Ana"])]));
        // A kind of name the limits don't mention passes.
        let only_dns = NameLimits { permitted: vec![GenName::Dns("example.com".into())], excluded: vec![] };
        assert!(only_dns.allow(&[dir(&[b"CN=Anyone"])]));
        let excluded = NameLimits { permitted: vec![], excluded: vec![dir(&[b"O=Acme"])] };
        assert!(!excluded.allow(&[dir(&[b"o=ACME", b"CN=Ana"])]));
    }
}
