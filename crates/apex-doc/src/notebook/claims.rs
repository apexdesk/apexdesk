//! Questions and hypotheses: pages with a status (open, supported,
//! refuted), weighed by the pages that link to them saying they support
//! or contradict them.

use super::{Notebook, Rel};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimKind {
    Question,
    Hypothesis,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Open,
    Supported,
    Refuted,
}

impl Status {
    pub const ALL: [Status; 3] = [Status::Open, Status::Supported, Status::Refuted];
}

/// What a question or a hypothesis page is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Claim {
    pub kind: ClaimKind,
    pub status: Status,
}

impl Claim {
    pub fn new(kind: ClaimKind) -> Claim {
        Claim { kind, status: Status::Open }
    }

    /// As kept in the file ("question:open").
    pub fn key(self) -> String {
        let kind = match self.kind {
            ClaimKind::Question => "question",
            ClaimKind::Hypothesis => "hypothesis",
        };
        let status = match self.status {
            Status::Open => "open",
            Status::Supported => "supported",
            Status::Refuted => "refuted",
        };
        format!("{kind}:{status}")
    }

    /// From [`Claim::key`]; anything else is no claim.
    pub fn from_key(s: &str) -> Option<Claim> {
        let (kind, status) = s.split_once(':')?;
        let kind = match kind {
            "question" => ClaimKind::Question,
            "hypothesis" => ClaimKind::Hypothesis,
            _ => return None,
        };
        let status = match status {
            "supported" => Status::Supported,
            "refuted" => Status::Refuted,
            _ => Status::Open,
        };
        Some(Claim { kind, status })
    }
}

/// The pages for and against a claim.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evidence {
    pub supports: Vec<i64>,
    pub contradicts: Vec<i64>,
}

impl Notebook {
    /// The live pages that say they support or contradict page `id`.
    pub fn evidence(&self, id: i64) -> Evidence {
        let mut e = Evidence::default();
        for (from, rel) in self.backlinks(id) {
            match rel {
                Rel::Supports => e.supports.push(from),
                Rel::Contradicts => e.contradicts.push(from),
                _ => {}
            }
        }
        e
    }

    /// The live questions and hypotheses, in notebook order.
    pub fn claims(&self) -> Vec<i64> {
        self.live_sections().iter().flat_map(|s| self.tree(s.id)).filter(|(_, p)| p.claim.is_some()).map(|(_, p)| p.id).collect()
    }
}
