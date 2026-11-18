//! Word banks and sentence templates for the retrieval-v2 generator (#788).
//!
//! Vocabulary lives in code, not in `scenario.json`, so a template change shows up as drift in the
//! checked-in fixture (ADR-0788). Placeholders: `{e}` entity, `{v}` value, `{kq}` kind (question
//! form), `{dq}` domain (question form), `{p}` project.
//!
//! Paraphrase discipline: the *note* wording of a domain, kind or attribute never shares a content
//! word with its *question* wording, so a paraphrase question is lexically far from its source.

/// How a value is drawn (see `world::draw_value`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Val {
    Money(u32, u32),
    Count(u32, u32),
    Month,
    Date,
    Person,
    City,
    Cadence,
    Percent,
    Vendor,
    /// A score with one decimal, e.g. `7.4`.
    Score,
}

pub struct KindDef {
    pub slug: &'static str,
    /// How the note calls this kind of thing.
    pub note: &'static str,
    /// How a paraphrase question calls it (no shared content word with `note`).
    pub q: &'static str,
}

pub const KINDS: [KindDef; 4] = [
    KindDef {
        slug: "project",
        note: "project",
        q: "initiative",
    },
    KindDef {
        slug: "vendor",
        note: "supplier",
        q: "outside provider",
    },
    KindDef {
        slug: "person",
        note: "team member",
        q: "employee",
    },
    KindDef {
        slug: "policy",
        note: "policy",
        q: "rule",
    },
];

/// Suffix that turns a name word into an entity name, per kind (persons use first + last names).
pub const NAME_SUFFIX: [&str; 4] = ["Core", "Supply", "", "Policy"];

/// `(note form, question form)`; no shared content word.
pub struct Domain {
    pub note: &'static str,
    pub q: &'static str,
}

pub const DOMAINS: [Domain; 12] = [
    Domain {
        note: "billing",
        q: "invoicing and payment collection",
    },
    Domain {
        note: "scheduling",
        q: "shift rostering",
    },
    Domain {
        note: "routing",
        q: "path planning for vehicles",
    },
    Domain {
        note: "telemetry",
        q: "sensor data streaming",
    },
    Domain {
        note: "onboarding",
        q: "new-hire induction",
    },
    Domain {
        note: "inventory",
        q: "stock tracking",
    },
    Domain {
        note: "support",
        q: "customer helpdesk work",
    },
    Domain {
        note: "analytics",
        q: "reporting dashboards",
    },
    Domain {
        note: "security",
        q: "access control",
    },
    Domain {
        note: "procurement",
        q: "purchasing",
    },
    Domain {
        note: "compliance",
        q: "regulatory audits",
    },
    Domain {
        note: "maintenance",
        q: "equipment servicing",
    },
];

pub const WORDS: [&str; 12] = [
    "Cedar", "Marlin", "Quill", "Tern", "Fable", "Orbit", "Juniper", "Helix", "Lumen", "Basalt",
    "Sable", "Nimbus",
];
pub const FIRST: [&str; 12] = [
    "Ines", "Tomas", "Priya", "Marcus", "Aiko", "Lena", "Diego", "Noor", "Callum", "Sofia", "Bram",
    "Yara",
];
pub const LAST: [&str; 12] = [
    "Varga",
    "Okafor",
    "Lindqvist",
    "Moreau",
    "Tanaka",
    "Haddad",
    "Brennan",
    "Kowalski",
    "Ferreira",
    "Nakamura",
    "Osei",
    "Petrov",
];
pub const CITIES: [&str; 12] = [
    "Lisbon",
    "Gdansk",
    "Rotterdam",
    "Leeds",
    "Porto",
    "Turin",
    "Aarhus",
    "Ghent",
    "Bergen",
    "Valencia",
    "Cork",
    "Tampere",
];
pub const CADENCES: [&str; 6] = [
    "monthly",
    "quarterly",
    "twice a year",
    "annually",
    "every six weeks",
    "every other month",
];
pub const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// One attribute of an entity kind.
pub struct AttrDef {
    pub key: &'static str,
    pub kind: usize,
    /// Label used in derived-page bullets.
    pub label: &'static str,
    /// Words that mark this attribute in note text (used to prove a negative is unanswerable).
    pub neg: &'static [&'static str],
    pub stmts: &'static [&'static str],
    pub qd: &'static [&'static str],
    pub qp: &'static [&'static str],
    /// Multi-hop question about a vendor attribute reached through a project (`{p}`).
    pub qhop: &'static str,
    pub val: Val,
}

pub const ATTRS: [AttrDef; 16] = [
    // ── project (0) ──
    AttrDef {
        key: "budget",
        kind: 0,
        label: "Budget",
        neg: &["budget"],
        stmts: &[
            "{e} runs on a monthly budget of {v}.",
            "The agreed budget for {e} is {v} a month.",
            "Finance capped the {e} budget at {v} per month.",
        ],
        qd: &[
            "What is the monthly budget for {e}?",
            "How much is the monthly budget of {e}?",
        ],
        qp: &["What is the spending ceiling for the {kq} that deals with {dq}?"],
        qhop: "",
        val: Val::Money(2000, 9000),
    },
    AttrDef {
        key: "owner",
        kind: 0,
        label: "Owner",
        neg: &["owned by"],
        stmts: &[
            "{e} is owned by {v}.",
            "{v} is the accountable owner of {e}.",
            "Ownership of {e} sits with {v}.",
        ],
        qd: &["Who owns {e}?", "Who is the owner of {e}?"],
        qp: &["Who is answerable for the {kq} that deals with {dq}?"],
        qhop: "",
        val: Val::Person,
    },
    AttrDef {
        key: "launch",
        kind: 0,
        label: "Launch",
        neg: &["launch"],
        stmts: &[
            "{e} is scheduled to launch in {v}.",
            "The launch target for {e} is {v}.",
            "{e} launches in {v}.",
        ],
        qd: &[
            "When does {e} launch?",
            "What is the launch target for {e}?",
        ],
        qp: &["When is the go-live for the {kq} that deals with {dq}?"],
        qhop: "",
        val: Val::Month,
    },
    AttrDef {
        key: "team",
        kind: 0,
        label: "Team size",
        neg: &["engineers"],
        stmts: &[
            "{e} has {v} engineers assigned.",
            "The {e} team consists of {v} engineers.",
            "{v} engineers work on {e}.",
        ],
        qd: &[
            "How many engineers work on {e}?",
            "How many engineers are on the {e} team?",
        ],
        qp: &["What is the staffing level of the {kq} that deals with {dq}?"],
        qhop: "",
        val: Val::Count(3, 14),
    },
    AttrDef {
        key: "dependency",
        kind: 0,
        label: "Supplier",
        neg: &["depends on", "supplier relationship"],
        stmts: &[
            "{e} depends on {v} for supply.",
            "{v} is the supplier behind {e}.",
            "The supplier relationship for {e} is with {v}.",
        ],
        qd: &["Which supplier does {e} depend on?"],
        qp: &["Which outside company feeds the {kq} that deals with {dq}?"],
        qhop: "",
        val: Val::Vendor,
    },
    // ── vendor (1) ──
    AttrDef {
        key: "price",
        kind: 1,
        label: "Contract price",
        neg: &["per unit"],
        stmts: &[
            "{e} charges {v} per unit under the current contract.",
            "The contract price with {e} is {v} per unit.",
            "Under contract, {e} bills {v} for each unit.",
        ],
        qd: &[
            "What price per unit does {e} charge?",
            "What is the contract price with {e}?",
        ],
        qp: &["What is the unit cost charged by the {kq} that deals with {dq}?"],
        qhop: "What price per unit does the supplier of {p} charge?",
        val: Val::Money(12, 95),
    },
    AttrDef {
        key: "renewal",
        kind: 1,
        label: "Renewal",
        neg: &["renew"],
        stmts: &[
            "The {e} contract renews on {v}.",
            "{e} renews on {v}.",
            "Renewal of the {e} agreement falls on {v}.",
        ],
        qd: &["When does the {e} contract renew?"],
        qp: &["When does the arrangement lapse with the {kq} that deals with {dq}?"],
        qhop: "When does the contract with the supplier of {p} renew?",
        val: Val::Date,
    },
    AttrDef {
        key: "contact",
        kind: 1,
        label: "Account manager",
        neg: &["account manager"],
        stmts: &[
            "Our account manager at {e} is {v}.",
            "{v} is the {e} account manager.",
            "For {e}, escalate to {v}, the account manager.",
        ],
        qd: &["Who is the account manager at {e}?"],
        qp: &["Who is our point of contact at the {kq} that deals with {dq}?"],
        qhop: "Who is the account manager at the supplier that {p} depends on?",
        val: Val::Person,
    },
    AttrDef {
        key: "uptime",
        kind: 1,
        label: "Uptime SLA",
        neg: &["uptime"],
        stmts: &[
            "{e} commits to {v} uptime.",
            "The {e} service level promises {v} uptime.",
            "{e} guarantees {v} uptime in the SLA.",
        ],
        qd: &["What uptime does {e} commit to?"],
        qp: &["What reliability promise does the {kq} that deals with {dq} make?"],
        qhop: "What uptime does the supplier behind {p} commit to?",
        val: Val::Percent,
    },
    // ── person (2) ──
    AttrDef {
        key: "city",
        kind: 2,
        label: "Location",
        neg: &["based in", "works out of", "home office"],
        stmts: &[
            "{e} works out of {v}.",
            "{e} is based in {v}.",
            "The home office of {e} is {v}.",
        ],
        qd: &["Where is {e} based?", "Which city does {e} work from?"],
        qp: &["Which town does the {kq} who works on {dq} call home?"],
        qhop: "",
        val: Val::City,
    },
    AttrDef {
        key: "start",
        kind: 2,
        label: "Joined",
        neg: &["joined", "started at"],
        stmts: &[
            "{e} joined in {v}.",
            "{e} started at the company in {v}.",
            "The start date for {e} is {v}.",
        ],
        qd: &["When did {e} join?", "When did {e} start at the company?"],
        qp: &["When did the {kq} who works on {dq} come aboard?"],
        qhop: "",
        val: Val::Month,
    },
    AttrDef {
        key: "manager",
        kind: 2,
        label: "Reports to",
        neg: &["reports to", "line manager", "manages"],
        stmts: &[
            "{e} reports to {v}.",
            "The line manager of {e} is {v}.",
            "{v} manages {e}.",
        ],
        qd: &["Who does {e} report to?", "Who is the line manager of {e}?"],
        qp: &["Who supervises the {kq} who works on {dq}?"],
        qhop: "",
        val: Val::Person,
    },
    AttrDef {
        key: "rota",
        kind: 2,
        label: "Pager rota",
        neg: &["pager"],
        stmts: &[
            "{e} carries the pager every {v} days.",
            "The pager rota for {e} comes round every {v} days.",
            "Every {v} days, {e} takes the pager.",
        ],
        qd: &["How often does {e} carry the pager?"],
        qp: &["What is the turnaround of the on-call shifts for the {kq} who works on {dq}?"],
        qhop: "",
        val: Val::Count(5, 21),
    },
    // ── policy (3) ──
    AttrDef {
        key: "window",
        kind: 3,
        label: "Request window",
        neg: &["window", "submit"],
        stmts: &[
            "The {e} allows {v} days to submit a request.",
            "{e}: requests must arrive within {v} days.",
            "Under the {e}, the submission window is {v} days.",
        ],
        qd: &["How many days is the request window under the {e}?"],
        qp: &["What is the cut-off in days for the {kq} governing {dq}?"],
        qhop: "",
        val: Val::Count(5, 60),
    },
    AttrDef {
        key: "approver",
        kind: 3,
        label: "Exception approver",
        neg: &["approves", "sign-off", "approver"],
        stmts: &[
            "{v} approves exceptions to the {e}.",
            "Exceptions to the {e} need sign-off from {v}.",
            "The {e} exception approver is {v}.",
        ],
        qd: &["Who approves exceptions to the {e}?"],
        qp: &["Who grants waivers for the {kq} governing {dq}?"],
        qhop: "",
        val: Val::Person,
    },
    AttrDef {
        key: "review",
        kind: 3,
        label: "Review cadence",
        neg: &["review cadence"],
        stmts: &[
            "The {e} is reviewed {v}.",
            "{e} gets a {v} review.",
            "Review cadence for the {e}: {v}.",
        ],
        qd: &["How often is the {e} reviewed?"],
        qp: &["How frequently is the {kq} governing {dq} revisited?"],
        qhop: "",
        val: Val::Cadence,
    },
];

/// Facts that appear only buried inside long notes (passage questions). Projects and persons only.
pub struct LogAttr {
    pub key: &'static str,
    pub stmts: &'static [&'static str],
    pub qd: &'static str,
    pub val: Val,
}

pub const LOG_ATTRS: [LogAttr; 8] = [
    LogAttr {
        key: "retro",
        stmts: &["{e} held its quarterly retrospective on {v}."],
        qd: "When did {e} hold its quarterly retrospective?",
        val: Val::Date,
    },
    LogAttr {
        key: "incidents",
        stmts: &["{e} logged {v} incidents during the quarter."],
        qd: "How many incidents did {e} log during the quarter?",
        val: Val::Count(2, 48),
    },
    LogAttr {
        key: "backlog",
        stmts: &["The {e} backlog stood at {v} open tickets at the end of the sprint."],
        qd: "How many open tickets were in the {e} backlog at the end of the sprint?",
        val: Val::Count(11, 240),
    },
    LogAttr {
        key: "demo",
        stmts: &["{v} people attended the {e} demo."],
        qd: "How many people attended the {e} demo?",
        val: Val::Count(6, 90),
    },
    LogAttr {
        key: "training",
        stmts: &["{e} completed {v} hours of training this term."],
        qd: "How many hours of training did {e} complete this term?",
        val: Val::Count(4, 60),
    },
    LogAttr {
        key: "audit",
        stmts: &["{e} scored {v} out of 100 in the internal audit."],
        qd: "What did {e} score out of 100 in the internal audit?",
        val: Val::Count(61, 99),
    },
    LogAttr {
        key: "offsite",
        stmts: &["The {e} offsite will take place in {v}."],
        qd: "Where will the {e} offsite take place?",
        val: Val::City,
    },
    LogAttr {
        key: "survey",
        stmts: &["{e} received a satisfaction rating of {v} out of 10."],
        qd: "What satisfaction rating out of 10 did {e} receive?",
        val: Val::Score,
    },
];

/// A revisable decision about a project.
pub struct ChainTopic {
    pub slug: &'static str,
    /// Topic as written in notes and direct questions.
    pub label: &'static str,
    pub values: &'static [&'static str],
}

pub const CHAIN_TOPICS: [ChainTopic; 10] = [
    ChainTopic {
        slug: "hosting",
        label: "hosting provider",
        values: &[
            "Harbor Hosting",
            "Tern Hosting",
            "Delta Cloud",
            "Pylon Metal",
            "Stratus One",
            "Kiln Compute",
        ],
    },
    ChainTopic {
        slug: "tracker",
        label: "issue tracker",
        values: &[
            "Trackly",
            "Ticketwell",
            "Boardwise",
            "Pinpoint",
            "Kanbo",
            "Stackdesk",
        ],
    },
    ChainTopic {
        slug: "payments",
        label: "payment processor",
        values: &[
            "Paywell",
            "Coinhaven",
            "Ledgerline",
            "Tillpoint",
            "Remitly One",
            "Fastcharge",
        ],
    },
    ChainTopic {
        slug: "office",
        label: "office location",
        values: &[
            "Pier Street",
            "Mill Lane",
            "Foundry Row",
            "Canal Quarter",
            "Station Yard",
            "Orchard Road",
        ],
    },
    ChainTopic {
        slug: "release",
        label: "release cadence",
        values: &[
            "weekly",
            "fortnightly",
            "monthly",
            "every six weeks",
            "quarterly",
        ],
    },
    ChainTopic {
        slug: "support-hours",
        label: "support hours",
        values: &[
            "8am to 4pm",
            "9am to 5pm",
            "7am to 7pm",
            "around the clock",
            "10am to 6pm",
        ],
    },
    ChainTopic {
        slug: "datastore",
        label: "primary database",
        values: &[
            "Postgres",
            "MariaDB",
            "DynamoDB",
            "Cockroach",
            "Mongo",
            "Redis",
        ],
    },
    ChainTopic {
        slug: "design",
        label: "design tool",
        values: &["Figma", "Penpot", "Sketchbook", "Canvas Studio", "Inkwell"],
    },
    ChainTopic {
        slug: "alerting",
        label: "alerting channel",
        values: &[
            "PagerHub",
            "OpsBell",
            "the Slack alerts room",
            "an email digest",
            "an SMS gateway",
        ],
    },
    ChainTopic {
        slug: "region",
        label: "cloud region",
        values: &["eu-west", "eu-north", "us-east", "us-west", "ap-south"],
    },
];

pub const REASONS: [&str; 10] = [
    "The previous setup kept causing friction for the team.",
    "A cheaper option turned up after the last review.",
    "Several people asked for this at the retrospective.",
    "The old choice did not scale with our growth.",
    "Support responsiveness was the deciding factor.",
    "It fits better with how we already work.",
    "A trial over two weeks went well.",
    "Compliance asked for a change.",
    "We were tired of the migration risk hanging over us.",
    "The numbers simply looked better.",
];

pub const DAYS: [&str; 5] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"];
pub const NOUNS: [&str; 24] = [
    "backlog",
    "agenda",
    "checklist",
    "roadmap",
    "draft",
    "proposal",
    "rota",
    "timeline",
    "handover",
    "report",
    "workshop",
    "survey",
    "plan",
    "inventory",
    "outline",
    "briefing",
    "calendar",
    "template",
    "spreadsheet",
    "summary",
    "wishlist",
    "handbook",
    "playbook",
    "glossary",
];
pub const VERBS: [&str; 16] = [
    "tidy", "revisit", "split", "merge", "park", "postpone", "simplify", "rewrite", "archive",
    "escalate", "document", "pilot", "trim", "reorder", "clarify", "delegate",
];
pub const ADJS: [&str; 16] = [
    "stale",
    "overdue",
    "noisy",
    "tentative",
    "ambitious",
    "cramped",
    "familiar",
    "fragile",
    "optional",
    "recurring",
    "half-finished",
    "unglamorous",
    "tricky",
    "lightweight",
    "sprawling",
    "urgent",
];
pub const TOPICS: [&str; 20] = [
    "coffee machine rota",
    "visitor badges",
    "the quarterly newsletter",
    "desk allocation",
    "the shared drive layout",
    "meeting-free afternoons",
    "the holiday calendar",
    "lunch-and-learn sessions",
    "printer supplies",
    "the lobby plants",
    "the intranet homepage",
    "bike storage",
    "the welcome pack",
    "team photo day",
    "the recycling scheme",
    "slide template cleanup",
    "the old wiki archive",
    "conference room naming",
    "expense report reminders",
    "laptop refresh timing",
];

/// Generic workplace sentences. They never name an entity and avoid every domain word.
pub const FILLER: [&str; 30] = [
    "On {day} we walked through the {adj} {noun} and agreed to {verb} the first few items.",
    "Most of the discussion was about {topic}, which nobody wanted to own but everybody had opinions on.",
    "We decided to {verb} the {noun} before the next planning session.",
    "Someone suggested that the {adj} {noun} might be better handled as a short workshop.",
    "There was a brief tangent on {topic}; we parked it for later.",
    "The {noun} is still {adj}, so we set aside an hour on {day} to {verb} it.",
    "Nothing dramatic happened this week, which is the best kind of week.",
    "We spent the first ten minutes on {topic} and then moved on to the {noun}.",
    "A few people were out, so we kept the agenda short and left the {adj} items for next time.",
    "The group agreed to {verb} the {noun} and report back on {day}.",
    "It was noted that {topic} has come up three times now and deserves a proper answer.",
    "We re-read the {noun} together; it is less {adj} than we feared.",
    "Action item: {verb} the {noun} before {day}.",
    "The mood was upbeat, mostly because the {adj} {noun} finally got closed.",
    "A side conversation about {topic} ran long, but it was useful.",
    "We agreed that the {noun} should live in one place instead of three.",
    "Feedback on the {noun} was mostly positive, with one {adj} exception.",
    "Reminder to everyone to {verb} their own {noun} before the end of the month.",
    "The facilitator rotated again; this week's notes are a bit more {adj} than usual.",
    "We ran out of time before reaching {topic}, so it carries over.",
    "Several small things were ticked off the {noun}, which felt good.",
    "There is still no clear owner for {topic}, so we will raise it at the next sync.",
    "The {adj} {noun} was sent round for comments and nobody objected.",
    "We keep returning to {topic}; maybe it needs a decision rather than another discussion.",
    "On {day} the {noun} was reviewed line by line and trimmed down.",
    "A quick poll showed most of us would rather {verb} the {noun} than add another meeting.",
    "The notes from last time were {adj} but good enough to work from.",
    "We closed with a short round of thanks and a reminder about {topic}.",
    "Everyone agreed to keep the {noun} short and to {verb} anything that is not pulling its weight.",
    "The {noun} will be revisited once the {adj} backlog has been cleared.",
];

pub const HEADINGS: [&str; 16] = [
    "Notes",
    "Discussion",
    "Follow-ups",
    "Loose ends",
    "Observations",
    "Carry-overs",
    "Open items",
    "Context",
    "Housekeeping",
    "Side threads",
    "Decisions and non-decisions",
    "Odds and ends",
    "Looking ahead",
    "What changed",
    "Retrospective",
    "Reading list",
];

pub const DIGEST_TITLES: [&str; 8] = [
    "Weekly digest",
    "Working notes",
    "Team log",
    "Planning notes",
    "Sync notes",
    "Journal",
    "Operations diary",
    "Review notes",
];

pub const CLIP_TOPICS: [(&str, &str); 12] = [
    (
        "fleet-maintenance",
        "How fleet operators schedule preventive maintenance",
    ),
    (
        "cooperative-governance",
        "What small cooperatives get wrong about governance",
    ),
    ("remote-onboarding", "A field guide to remote team rituals"),
    (
        "pricing-experiments",
        "Three pricing experiments that did not work",
    ),
    (
        "vendor-risk",
        "Why single-supplier setups keep failing quietly",
    ),
    ("incident-reviews", "Blameless incident reviews in practice"),
    (
        "data-retention",
        "Retention schedules for small organisations",
    ),
    (
        "hiring-pipelines",
        "Building a hiring pipeline without a recruiter",
    ),
    (
        "customer-interviews",
        "Customer interviews: the questions that matter",
    ),
    (
        "procurement-basics",
        "A plain-language guide to purchasing for startups",
    ),
    (
        "urban-mobility",
        "The state of urban mobility in mid-sized cities",
    ),
    (
        "documentation-habits",
        "Documentation habits of teams that ship",
    ),
];

pub const CLIP_SENTENCES: [&str; 14] = [
    "The author argues that most teams over-engineer the first version of a process.",
    "According to the piece, a small checklist beats a long policy nearly every time.",
    "One interviewee described the shift as boring at first and transformative later.",
    "The article cites a survey of mid-sized operators, though the sample is small.",
    "A recurring theme is that decisions should be written down where people will find them.",
    "The writer is sceptical of dashboards that nobody opens after the first month.",
    "Case studies in the second half show mixed results, with a clear pattern around ownership.",
    "The conclusion recommends starting small and reviewing quarterly.",
    "Several readers pushed back in the comments, mostly on the cost estimates.",
    "The piece closes with a reading list and an invitation to share experiences.",
    "Industry analysts quoted here disagree on how fast the market will consolidate.",
    "The author notes that the best-run teams treat documentation as a product.",
    "A sidebar compares three common approaches and their trade-offs.",
    "The second case study is the most convincing, largely because it includes the failures.",
];
