//! Planning (Insert > Planning): project schedules inside the sheet.
//!
//! A task plan is a table (see [`crate::table`]) whose columns have known
//! roles ([`Field`]): name, duration, start, finish, predecessors… Its
//! cells hold plain values, so other programs see an ordinary table; the
//! calendars and the settings are [`Plan`], kept with the table. The
//! outline of the tasks is the indent of their name cells. A resource
//! sheet is another table with resource fields.
//!
//! The scheduler ([`engine`]) knows nothing of sheets: `workbook/plan.rs`
//! reads the table into [`engine::TaskIn`]s and writes the dates back.

pub mod calendar;
pub mod earned;
pub mod engine;
pub mod holidays;
pub mod level;
pub mod parse;

pub use calendar::{Calendar, Exception, WorkTime};
pub use engine::{Schedule, TaskIn, TaskOut, Warning, schedule};

/// Dependency types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DepType {
    /// Finish-to-start: the task starts when its predecessor finishes.
    FS,
    SS,
    FF,
    SF,
}

impl DepType {
    pub const ALL: [DepType; 4] = [DepType::FS, DepType::SS, DepType::FF, DepType::SF];

    pub fn code(self) -> &'static str {
        match self {
            DepType::FS => "FS",
            DepType::SS => "SS",
            DepType::FF => "FF",
            DepType::SF => "SF",
        }
    }
}

/// Lag (positive) or lead (negative) of a link.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lag {
    /// Working days.
    Days(f64),
    /// Calendar days.
    Elapsed(f64),
    /// Percent of the predecessor's duration.
    Percent(f64),
}

/// A link from a predecessor (task index).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub pred: usize,
    pub kind: DepType,
    pub lag: Lag,
}

/// Date constraints (dates are serial days).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Constraint {
    #[default]
    Asap,
    Alap,
    /// Start no earlier than.
    Snet(f64),
    /// Start no later than.
    Snlt(f64),
    /// Finish no earlier than.
    Fnet(f64),
    /// Finish no later than.
    Fnlt(f64),
    /// Must start on.
    Mso(f64),
    /// Must finish on.
    Mfo(f64),
}

impl Constraint {
    /// Keys of the kinds (files and the constraint column's codes).
    pub const KEYS: [&'static str; 8] = ["asap", "alap", "snet", "snlt", "fnet", "fnlt", "mso", "mfo"];

    pub fn key(self) -> &'static str {
        Self::KEYS[self.index()]
    }

    pub fn index(self) -> usize {
        match self {
            Constraint::Asap => 0,
            Constraint::Alap => 1,
            Constraint::Snet(_) => 2,
            Constraint::Snlt(_) => 3,
            Constraint::Fnet(_) => 4,
            Constraint::Fnlt(_) => 5,
            Constraint::Mso(_) => 6,
            Constraint::Mfo(_) => 7,
        }
    }

    pub fn date(self) -> Option<f64> {
        match self {
            Constraint::Asap | Constraint::Alap => None,
            Constraint::Snet(d) | Constraint::Snlt(d) | Constraint::Fnet(d) | Constraint::Fnlt(d) | Constraint::Mso(d) | Constraint::Mfo(d) => Some(d),
        }
    }

    /// The kind `index` (see `KEYS`) on `date` (ignored by the first two).
    pub fn from_index(index: usize, date: f64) -> Constraint {
        match index {
            1 => Constraint::Alap,
            2 => Constraint::Snet(date),
            3 => Constraint::Snlt(date),
            4 => Constraint::Fnet(date),
            5 => Constraint::Fnlt(date),
            6 => Constraint::Mso(date),
            7 => Constraint::Mfo(date),
            _ => Constraint::Asap,
        }
    }

    /// The kind written in a cell: a key ("snet") or an English or local
    /// abbreviation.
    pub fn parse_kind(s: &str) -> Option<usize> {
        let s = s.trim().to_lowercase();
        if s.is_empty() {
            return Some(0);
        }
        if let Some(i) = Self::KEYS.iter().position(|k| *k == s) {
            return Some(i);
        }
        // Spanish, French, German, Italian, Portuguese abbreviations.
        const LOCAL: [(&str, usize); 16] = [("lap", 0), ("ltp", 1), ("nat", 2), ("ndt", 3), ("fnat", 4), ("fndt", 5), ("dce", 6), ("dfe", 7), ("pdp", 0), ("ptp", 1), ("ang", 2), ("ens", 3), ("mae", 6), ("mfe", 7), ("sff", 0), ("stp", 1)];
        LOCAL.iter().find(|(k, _)| *k == s).map(|(_, i)| *i)
    }
}

/// The role of a column of a plan table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    // Tasks.
    Id,
    Wbs,
    Name,
    Duration,
    Start,
    Finish,
    Predecessors,
    Resources,
    Percent,
    Notes,
    Mode,
    Constraint,
    ConstraintDate,
    Priority,
    FixedCost,
    Cost,
    Work,
    Slack,
    ActualStart,
    ActualFinish,
    BaselineStart,
    BaselineFinish,
    BaselineCost,
    StartVariance,
    FinishVariance,
    CostVariance,
    LevelDelay,
    // Earned value at the status date.
    PlannedValue,
    EarnedValue,
    ActualCost,
    ScheduleVarianceEv,
    CostVarianceEv,
    Spi,
    Cpi,
    Eac,
    Vac,
    // Resources.
    ResName,
    ResType,
    ResCapacity,
    ResCalendar,
    ResRate,
    ResPerUse,
}

impl Field {
    pub const ALL: [Field; 42] = [
        Field::Id, Field::Wbs, Field::Name, Field::Duration, Field::Start, Field::Finish, Field::Predecessors, Field::Resources, Field::Percent, Field::Notes, Field::Mode, Field::Constraint, Field::ConstraintDate, Field::Priority, Field::FixedCost, Field::Cost, Field::Work, Field::Slack, Field::ActualStart, Field::ActualFinish,
        Field::BaselineStart, Field::BaselineFinish, Field::BaselineCost, Field::StartVariance, Field::FinishVariance, Field::CostVariance, Field::LevelDelay, Field::PlannedValue, Field::EarnedValue, Field::ActualCost, Field::ScheduleVarianceEv, Field::CostVarianceEv, Field::Spi, Field::Cpi, Field::Eac, Field::Vac, Field::ResName, Field::ResType, Field::ResCapacity, Field::ResCalendar, Field::ResRate, Field::ResPerUse,
    ];

    /// The columns of a new task plan.
    pub const TASKS: [Field; 9] = [Field::Wbs, Field::Name, Field::Duration, Field::Start, Field::Finish, Field::Predecessors, Field::Resources, Field::Percent, Field::Notes];

    /// The columns of a new resource sheet.
    pub const RESOURCES: [Field; 6] = [Field::ResName, Field::ResType, Field::ResCapacity, Field::ResCalendar, Field::ResRate, Field::ResPerUse];

    /// The name in files and the i18n key suffix (`plan.field.<key>`).
    pub fn key(self) -> &'static str {
        match self {
            Field::Id => "id",
            Field::Wbs => "wbs",
            Field::Name => "name",
            Field::Duration => "duration",
            Field::Start => "start",
            Field::Finish => "finish",
            Field::Predecessors => "predecessors",
            Field::Resources => "resources",
            Field::Percent => "percent",
            Field::Notes => "notes",
            Field::Mode => "mode",
            Field::Constraint => "constraint",
            Field::ConstraintDate => "constraint_date",
            Field::Priority => "priority",
            Field::FixedCost => "fixed_cost",
            Field::Cost => "cost",
            Field::Work => "work",
            Field::Slack => "slack",
            Field::ActualStart => "actual_start",
            Field::ActualFinish => "actual_finish",
            Field::BaselineStart => "baseline_start",
            Field::BaselineFinish => "baseline_finish",
            Field::BaselineCost => "baseline_cost",
            Field::StartVariance => "start_variance",
            Field::FinishVariance => "finish_variance",
            Field::CostVariance => "cost_variance",
            Field::LevelDelay => "level_delay",
            Field::PlannedValue => "pv",
            Field::EarnedValue => "ev",
            Field::ActualCost => "ac",
            Field::ScheduleVarianceEv => "sv",
            Field::CostVarianceEv => "cv",
            Field::Spi => "spi",
            Field::Cpi => "cpi",
            Field::Eac => "eac",
            Field::Vac => "vac",
            Field::ResName => "res_name",
            Field::ResType => "res_type",
            Field::ResCapacity => "res_capacity",
            Field::ResCalendar => "res_calendar",
            Field::ResRate => "res_rate",
            Field::ResPerUse => "res_per_use",
        }
    }

    pub fn from_key(k: &str) -> Option<Field> {
        Field::ALL.into_iter().find(|f| f.key() == k)
    }

    /// English column name (the app puts the local one).
    pub fn english(self) -> &'static str {
        match self {
            Field::Id => "ID",
            Field::Wbs => "WBS",
            Field::Name => "Task",
            Field::Duration => "Duration",
            Field::Start => "Start",
            Field::Finish => "Finish",
            Field::Predecessors => "Predecessors",
            Field::Resources => "Resources",
            Field::Percent => "% Complete",
            Field::Notes => "Notes",
            Field::Mode => "Mode",
            Field::Constraint => "Constraint",
            Field::ConstraintDate => "Constraint Date",
            Field::Priority => "Priority",
            Field::FixedCost => "Fixed Cost",
            Field::Cost => "Cost",
            Field::Work => "Work",
            Field::Slack => "Total Slack",
            Field::ActualStart => "Actual Start",
            Field::ActualFinish => "Actual Finish",
            Field::BaselineStart => "Baseline Start",
            Field::BaselineFinish => "Baseline Finish",
            Field::BaselineCost => "Baseline Cost",
            Field::StartVariance => "Start Variance",
            Field::FinishVariance => "Finish Variance",
            Field::CostVariance => "Cost Variance",
            Field::LevelDelay => "Leveling Delay",
            Field::PlannedValue => "Planned Value",
            Field::EarnedValue => "Earned Value",
            Field::ActualCost => "Actual Cost",
            Field::ScheduleVarianceEv => "SV",
            Field::CostVarianceEv => "CV",
            Field::Spi => "SPI",
            Field::Cpi => "CPI",
            Field::Eac => "EAC",
            Field::Vac => "VAC",
            Field::ResName => "Resource",
            Field::ResType => "Type",
            Field::ResCapacity => "Max Units",
            Field::ResCalendar => "Calendar",
            Field::ResRate => "Rate",
            Field::ResPerUse => "Cost/Use",
        }
    }

    /// Filled in by the scheduler (typing there is not an input, except
    /// for start and finish, which set a constraint or the duration).
    pub fn computed(self) -> bool {
        matches!(self, Field::Id | Field::Wbs | Field::Cost | Field::Work | Field::Slack | Field::StartVariance | Field::FinishVariance | Field::CostVariance) || self.is_earned()
    }

    /// Earned value at the status date.
    pub fn is_earned(self) -> bool {
        matches!(self, Field::PlannedValue | Field::EarnedValue | Field::ActualCost | Field::ScheduleVarianceEv | Field::CostVarianceEv | Field::Spi | Field::Cpi | Field::Eac | Field::Vac)
    }

    /// Holds a date.
    pub fn is_date(self) -> bool {
        matches!(self, Field::Start | Field::Finish | Field::ConstraintDate | Field::ActualStart | Field::ActualFinish | Field::BaselineStart | Field::BaselineFinish)
    }

    /// Holds money.
    pub fn is_money(self) -> bool {
        matches!(self, Field::FixedCost | Field::Cost | Field::BaselineCost | Field::CostVariance | Field::ResRate | Field::ResPerUse) || self.is_earned() && !matches!(self, Field::Spi | Field::Cpi)
    }

    pub fn is_resource(self) -> bool {
        matches!(self, Field::ResName | Field::ResType | Field::ResCapacity | Field::ResCalendar | Field::ResRate | Field::ResPerUse)
    }
}

/// Resource kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ResKind {
    /// People and machines: their work takes time.
    #[default]
    Work,
    /// Consumed by quantity (the rate is per unit).
    Material,
    /// A cost that the assignment gives (travel).
    Cost,
}

impl ResKind {
    pub const KEYS: [&'static str; 3] = ["work", "material", "cost"];

    pub fn key(self) -> &'static str {
        Self::KEYS[self as usize]
    }

    /// A key, or the local word's first letters (trabajo, material, coste…).
    pub fn parse(s: &str) -> ResKind {
        let s = s.trim().to_lowercase();
        if s.starts_with("mat") {
            ResKind::Material
        } else if s.starts_with("cost") || s.starts_with("coût") || s.starts_with("kost") || s.starts_with("cust") {
            ResKind::Cost
        } else {
            ResKind::Work
        }
    }
}

/// What kind of plan table this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PlanKind {
    #[default]
    Tasks,
    Resources,
}

/// The settings of a plan table, kept as our extension in files.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub kind: PlanKind,
    /// Project start (serial day).
    pub start: f64,
    /// The project calendar first, then the resources' own.
    pub calendars: Vec<Calendar>,
    /// Name of the resource table (empty: none).
    pub resources: String,
    /// When the baseline was saved (serial), if it was.
    pub baseline: Option<f64>,
    /// Status date for progress (serial; None: today).
    pub status: Option<f64>,
    /// Columns of the user's with a formula over the task's fields or a
    /// roll-up into the summaries.
    pub custom: Vec<CustomField>,
}

/// How a summary task gets the value of a custom field from its subtasks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Rollup {
    /// Summaries compute the formula like any task (or keep what was typed).
    None,
    #[default]
    Sum,
    Average,
    Max,
    Min,
}

impl Rollup {
    pub const ALL: [Rollup; 5] = [Rollup::None, Rollup::Sum, Rollup::Average, Rollup::Max, Rollup::Min];

    /// The name in files and the i18n key suffix (`plan.rollup.<key>`).
    pub fn key(self) -> &'static str {
        match self {
            Rollup::None => "none",
            Rollup::Sum => "sum",
            Rollup::Average => "avg",
            Rollup::Max => "max",
            Rollup::Min => "min",
        }
    }

    pub fn from_key(k: &str) -> Rollup {
        Rollup::ALL.into_iter().find(|r| r.key() == k).unwrap_or_default()
    }

    /// The roll-up of `values` (None when there are none).
    pub fn apply(self, values: &[f64]) -> Option<f64> {
        if values.is_empty() {
            return None;
        }
        Some(match self {
            Rollup::None => return None,
            Rollup::Sum => values.iter().sum(),
            Rollup::Average => values.iter().sum::<f64>() / values.len() as f64,
            Rollup::Max => values.iter().copied().fold(f64::MIN, f64::max),
            Rollup::Min => values.iter().copied().fold(f64::MAX, f64::min),
        })
    }
}

/// A custom field: a column of the plan table, found by its name.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CustomField {
    pub column: String,
    /// A formula over the task's columns (`[Cost]*2`, `=[@Work]/8`);
    /// empty: the values are typed.
    pub formula: String,
    pub rollup: Rollup,
}

impl CustomField {
    /// The formula as the sheet reads it: `[Column]` means this task's
    /// value (`[@Column]`), as in a table's calculated column.
    pub fn source(&self) -> String {
        this_row_refs(self.formula.trim().trim_start_matches('='))
    }
}

/// `[Col]` and `[Two words]` as `[@Col]` and `[@[Two words]]`, outside
/// strings and after no table name.
pub fn this_row_refs(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 8);
    let mut i = 0;
    let mut quote = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            quote = !quote;
        }
        let after_name = out.chars().last().is_some_and(|p| p.is_alphanumeric() || p == '_' || p == ']' || p == '.');
        if c == '[' && !quote && !after_name
            && let Some(len) = chars[i + 1..].iter().position(|x| *x == ']')
        {
            let inner: String = chars[i + 1..i + 1 + len].iter().collect();
            if !inner.starts_with(['@', '#', '[']) && !inner.contains('[') {
                let simple = inner.chars().all(|x| x.is_alphanumeric() || x == '_' || x == '.');
                if simple {
                    out.push_str(&format!("[@{inner}]"));
                } else {
                    out.push_str(&format!("[@[{inner}]]"));
                }
                i += len + 2;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

impl Plan {
    pub fn tasks(start: f64) -> Plan {
        Plan { kind: PlanKind::Tasks, start, calendars: vec![Calendar::default()], resources: String::new(), baseline: None, status: None, custom: Vec::new() }
    }

    pub fn resources() -> Plan {
        Plan { kind: PlanKind::Resources, calendars: Vec::new(), ..Plan::tasks(0.0) }
    }

    /// The project calendar.
    pub fn calendar(&self) -> Calendar {
        self.calendars.first().cloned().unwrap_or_default()
    }

    pub fn hours_per_day(&self) -> f64 {
        self.calendars.first().map_or(8.0, |c| c.hours_per_day())
    }

    pub fn calendar_index(&self, name: &str) -> Option<usize> {
        let n = name.trim().to_lowercase();
        if n.is_empty() {
            return None;
        }
        self.calendars.iter().position(|c| c.name.to_lowercase() == n)
    }

    /// The custom field of the column named `name`.
    pub fn custom_of(&self, name: &str) -> Option<&CustomField> {
        self.custom.iter().find(|c| c.column.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests;
