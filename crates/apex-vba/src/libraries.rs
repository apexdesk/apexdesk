//! What ApexDesk runs of the libraries a project refers to (Tools >
//! References), by their GUID.

/// How much of a library macros can use here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// What macros use of it runs.
    Yes,
    /// Some of it: `note` says what.
    Partial(&'static str),
    /// Nothing of it: code that uses it stops with an error.
    No,
}

const LIBRARIES: &[(&str, Support)] = &[
    // Visual Basic For Applications, OLE Automation, Excel.
    ("{000204EF-0000-0000-C000-000000000046}", Support::Yes),
    ("{00020430-0000-0000-C000-000000000046}", Support::Yes),
    ("{00020813-0000-0000-C000-000000000046}", Support::Yes),
    // Microsoft Scripting Runtime.
    ("{420B2830-E718-11CF-893D-00A0C9054228}", Support::Partial("Dictionary, FileSystemObject")),
    // Microsoft Office Object Library.
    ("{2DF8D04C-5BFA-101B-BDE5-00AA0044DE52}", Support::Partial("FileDialog, msoFileDialog* and other constants")),
];

/// What runs of the library with `guid` (`{...}`, any case).
pub fn support(guid: &str) -> Support {
    LIBRARIES.iter().find(|(g, _)| g.eq_ignore_ascii_case(guid)).map_or(Support::No, |(_, s)| *s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_libraries() {
        assert_eq!(support("{00020813-0000-0000-c000-000000000046}"), Support::Yes);
        assert!(matches!(support("{420B2830-E718-11CF-893D-00A0C9054228}"), Support::Partial(_)));
        // Outlook.
        assert_eq!(support("{00062FFF-0000-0000-C000-000000000046}"), Support::No);
    }
}
