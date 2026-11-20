//! The numbered rule catalogue.
//!
//! A stable, cited, numbered rule set that a downstream system can reference in
//! a ticket: "V07 on the Frankfurt gateway" means the same thing to everyone
//! who reads it, this year and in five years.
//!
//! # What runs here, and what does not
//!
//! [`validate_records`] evaluates every rule that is decidable from a slice of
//! records plus an evaluation instant. Three rules are inherently **stateful** —
//! recognising a retransmission, spotting a missing file, completing a run of
//! partials — and those live in [`crate::dedup`], [`crate::sequence`] and
//! [`crate::partial`]. Their outcomes join the same catalogue through
//! [`Finding::duplicate`], [`Finding::sequence_gap`] and
//! [`Finding::reassembly`], so a report has one vocabulary rather than four.
//!
//! Keeping them apart is what lets `validate_records` be a pure function of its
//! arguments.
//!
//! # Two properties worth stating
//!
//! **Order independence.** The adjacency rule (V16) evaluates in instant order
//! whatever order you supply, so a shuffled input does not produce a cascade of
//! spurious findings. The disorder itself is reported once.
//!
//! **Findings point into your slice.** [`Finding::record_index`] refers to the
//! input you passed, not to an internal ordering, so a caller can go straight
//! from a finding to the record that produced it.
//!
//! ```rust
//! use mediation::{Rule, Severity, ValidationConfig, validate_records};
//! # use mediation::{ChargedParty, RecordType, Seconds, SourceRef, SubscriberRef, UsageRecord};
//! # use time::macros::datetime;
//! # let party = ChargedParty::Subscriber(
//! #     SubscriberRef::new("imsi-262021234567890".parse().unwrap()));
//! # let source = SourceRef::new("msc-01".parse().unwrap(), 0);
//! // A call billed for longer than it can possibly have lasted.
//! let record = UsageRecord::new(
//!     RecordType::OriginatedCall,
//!     party,
//!     "2026-07-28T10:00:00+02:00".parse().unwrap(),
//!     source,
//! )
//! .with_closed("2026-07-28T10:00:10+02:00".parse().unwrap())
//! .with_duration(Seconds::new(55));
//!
//! let report = validate_records(
//!     &[record],
//!     &ValidationConfig::default(),
//!     datetime!(2026-07-28 12:00:00 UTC),
//! );
//!
//! assert!(report.has(Rule::DurationExceedsSpan));
//! assert_eq!(report.severity_of(Rule::DurationExceedsSpan), Some(Severity::Blocking));
//! assert!(report.blocks_billing());
//! ```

use std::fmt;

use time::OffsetDateTime;

use crate::cdrtime::CdrTimestamp;
use crate::dedup::{NotKeyable, RecordKey};
use crate::jurisdiction::{Ambiguity, usage_country};
use crate::lateness::{Lateness, LatenessPolicy};
use crate::partial::{OrphanReason, Reassembly};
use crate::record::{PrimaryMeasure, ServiceDomain, UsageRecord};

// ── Rule ─────────────────────────────────────────────────────────────────────

/// One numbered rule.
///
/// The numbers are stable and are never reused: a rule that is retired keeps
/// its number reserved, so a five-year-old ticket citing V09 still means what
/// it meant. Adding a variant is a breaking change and ships as one — the same
/// reasoning as every other domain enum here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    /// **V01** — the record's UTC offset is outside the assigned range.
    UnassignedOffset,
    /// **V02** — the record opens after the evaluation instant.
    FutureTimestamp,
    /// **V03** — the record closes before it opens.
    ClosedBeforeOpened,
    /// **V04** — chargeable duration exceeds the wall-clock span.
    DurationExceedsSpan,
    /// **V05** — volume exceeds what the elapsed time could carry.
    ImplausibleVolume,
    /// **V06** — the record carries no quantity at all.
    ZeroUsage,
    /// **V07** — the container sum disagrees with the record's own total.
    ContainerSumMismatch,
    /// **V08** — no charged party could be determined.
    UnattributableParty,
    /// **V09** — the record has no natural key and cannot be deduplicated.
    NotKeyable,
    /// **V10** — the record is past its inclusion deadline.
    BeyondDeadline,
    /// **V11** — the record is late but still billable.
    LateArrival,
    /// **V12** — the record's serving network is absent (not raised for a
    /// wireline voice, IMS or fixed-access record, which has no PLMN).
    ServingNetworkAbsent,
    /// **V13** — the record's quantity does not match what its service measures.
    QuantityDoesNotMatchService,
    /// **V14** — an identifier's check digit does not verify.
    CheckDigitFailure,
    /// **V15** — the record's own volume overflows when its parts are summed.
    VolumeOverflow,
    /// **V16** — the input slice is not in ascending instant order.
    UnorderedSeries,
    /// **V17** — the record is a retransmission of one already seen.
    DuplicateRecord,
    /// **V18** — a source's sequence has an unfilled hole.
    ///
    /// A **warning**, and the one entry in the catalogue whose severity needs
    /// explaining. The hole itself is enforced elsewhere and harder: every
    /// record from that source is held
    /// ([`HoldReason::SequenceGap`](crate::HoldReason::SequenceGap)), because
    /// its completeness cannot be proved. This finding is the *evidence* — the
    /// files to ask the partner to resend — and blocking the batch on it as
    /// well would stop records from every other source, whose completeness is
    /// not in question. Escalate it if you would rather the whole batch waited.
    SequenceGap,
    /// **V19** — a run of partials cannot be reassembled at all.
    OrphanPartial,
    /// **V20** — a run of partials is missing members.
    IncompleteReassembly,
    /// **V21** — the serving network and the reported location name different
    /// countries.
    JurisdictionConflict,
}

impl Rule {
    /// Every rule, in catalogue order.
    pub const ALL: &'static [Self] = &[
        Self::UnassignedOffset,
        Self::FutureTimestamp,
        Self::ClosedBeforeOpened,
        Self::DurationExceedsSpan,
        Self::ImplausibleVolume,
        Self::ZeroUsage,
        Self::ContainerSumMismatch,
        Self::UnattributableParty,
        Self::NotKeyable,
        Self::BeyondDeadline,
        Self::LateArrival,
        Self::ServingNetworkAbsent,
        Self::QuantityDoesNotMatchService,
        Self::CheckDigitFailure,
        Self::VolumeOverflow,
        Self::UnorderedSeries,
        Self::DuplicateRecord,
        Self::SequenceGap,
        Self::OrphanPartial,
        Self::IncompleteReassembly,
        Self::JurisdictionConflict,
    ];

    /// The stable catalogue code — `V01` … `V21`.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnassignedOffset => "V01",
            Self::FutureTimestamp => "V02",
            Self::ClosedBeforeOpened => "V03",
            Self::DurationExceedsSpan => "V04",
            Self::ImplausibleVolume => "V05",
            Self::ZeroUsage => "V06",
            Self::ContainerSumMismatch => "V07",
            Self::UnattributableParty => "V08",
            Self::NotKeyable => "V09",
            Self::BeyondDeadline => "V10",
            Self::LateArrival => "V11",
            Self::ServingNetworkAbsent => "V12",
            Self::QuantityDoesNotMatchService => "V13",
            Self::CheckDigitFailure => "V14",
            Self::VolumeOverflow => "V15",
            Self::UnorderedSeries => "V16",
            Self::DuplicateRecord => "V17",
            Self::SequenceGap => "V18",
            Self::OrphanPartial => "V19",
            Self::IncompleteReassembly => "V20",
            Self::JurisdictionConflict => "V21",
        }
    }

    /// A one-line description, for a report a human reads.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::UnassignedOffset => "UTC offset outside the assigned range",
            Self::FutureTimestamp => "record opens after the evaluation instant",
            Self::ClosedBeforeOpened => "record closes before it opens",
            Self::DurationExceedsSpan => "chargeable duration exceeds the wall-clock span",
            Self::ImplausibleVolume => "volume exceeds what the elapsed time could carry",
            Self::ZeroUsage => "record carries no quantity",
            Self::ContainerSumMismatch => "container sum disagrees with the record total",
            Self::UnattributableParty => "no charged party could be determined",
            Self::NotKeyable => "no natural key, so the record cannot be deduplicated",
            Self::BeyondDeadline => "past the inclusion deadline",
            Self::LateArrival => "late, but still billable",
            Self::ServingNetworkAbsent => "serving network absent",
            Self::QuantityDoesNotMatchService => {
                "quantity does not match what the service measures"
            }
            Self::CheckDigitFailure => "identifier check digit does not verify",
            Self::VolumeOverflow => "volume overflows when summed",
            Self::UnorderedSeries => "input is not in ascending instant order",
            Self::DuplicateRecord => "retransmission of a record already seen",
            Self::SequenceGap => "source sequence has an unfilled hole",
            Self::OrphanPartial => "run of partials cannot be reassembled",
            Self::IncompleteReassembly => "run of partials is missing members",
            Self::JurisdictionConflict => {
                "serving network and reported location disagree on the country"
            }
        }
    }

    /// Whether a finding of this rule stops the record being billed.
    ///
    /// The default policy, which [`ValidationConfig`] can tighten but never
    /// loosen — see [`ValidationConfig::escalate`].
    #[must_use]
    pub const fn default_severity(self) -> Severity {
        match self {
            // Wrong quantities, wrong attribution, wrong completeness: billing
            // any of these invoices an amount that cannot be defended.
            Self::UnassignedOffset
            | Self::FutureTimestamp
            | Self::ClosedBeforeOpened
            | Self::DurationExceedsSpan
            | Self::ContainerSumMismatch
            | Self::UnattributableParty
            | Self::NotKeyable
            | Self::BeyondDeadline
            | Self::VolumeOverflow
            | Self::OrphanPartial
            | Self::IncompleteReassembly
            | Self::JurisdictionConflict => Severity::Blocking,
            // Suspicious, but the record may still be right.
            Self::SequenceGap
            | Self::ImplausibleVolume
            | Self::ZeroUsage
            | Self::LateArrival
            | Self::ServingNetworkAbsent
            | Self::QuantityDoesNotMatchService
            | Self::CheckDigitFailure
            | Self::UnorderedSeries
            | Self::DuplicateRecord => Severity::Warning,
        }
    }
}

impl fmt::Display for Rule {
    /// Writes `V07 container sum disagrees with the record total`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.code(), self.describe())
    }
}

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Worth an operator's attention; the record may still be billed.
    Warning,
    /// The record must not be billed as it stands.
    Blocking,
}

impl Severity {
    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Warning => "WARNING",
            Self::Blocking => "BLOCKING",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Finding ──────────────────────────────────────────────────────────────────

/// One rule firing on one record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Which rule fired.
    pub rule: Rule,
    /// How much it matters, after any configured escalation.
    pub severity: Severity,
    /// Which record in the **caller's** slice, when the rule is per-record.
    ///
    /// `None` for rules that describe a batch or a source rather than a record.
    pub record_index: Option<usize>,
    /// What the rule saw, for a report a human reads.
    pub detail: String,
}

impl Finding {
    /// A per-record finding at the rule's default severity.
    #[must_use]
    pub fn new(rule: Rule, record_index: usize, detail: impl Into<String>) -> Self {
        Self {
            rule,
            severity: rule.default_severity(),
            record_index: Some(record_index),
            detail: detail.into(),
        }
    }

    /// A batch-level finding, belonging to no single record.
    #[must_use]
    pub fn batch(rule: Rule, detail: impl Into<String>) -> Self {
        Self {
            rule,
            severity: rule.default_severity(),
            record_index: None,
            detail: detail.into(),
        }
    }

    /// The finding a [`crate::dedup::DedupDecision::Duplicate`] produces.
    ///
    /// Offered so a stateful outcome joins the same catalogue as the stateless
    /// rules, and a report speaks one vocabulary.
    #[must_use]
    ///
    /// The detail names the key. A [`RecordKey::Call`] is named by its kind and
    /// recording entity only: a SIP Call-ID often embeds the caller's number
    /// (`+4915…@host`), and finding text is routinely logged (§ 9 TDDDG).
    pub fn duplicate(record_index: usize, key: &RecordKey) -> Self {
        let detail = match key {
            RecordKey::Call { entity, .. } => format!("CALL:{entity}"),
            other => other.to_string(),
        };
        Self::new(Rule::DuplicateRecord, record_index, detail)
    }

    /// The finding an unkeyable record produces.
    #[must_use]
    pub fn not_keyable(record_index: usize, reason: NotKeyable) -> Self {
        Self::new(Rule::NotKeyable, record_index, reason.to_string())
    }

    /// The finding a [`crate::sequence::SequenceTracker`] hole produces.
    ///
    /// Batch-level: a hole belongs to a source, not to any record that arrived.
    ///
    /// `missing` is the bounded list of identities and `missing_count` the
    /// exact total, so a report says how bad the hole is even where naming
    /// every file in it would be a four-billion-element allocation.
    #[must_use]
    pub fn sequence_gap(source: &impl fmt::Display, missing: &[u32], missing_count: u64) -> Self {
        Self::batch(
            Rule::SequenceGap,
            if missing_count > missing.len() as u64 {
                format!("{source} missing {missing_count}, first {missing:?}")
            } else {
                format!("{source} missing {missing:?}")
            },
        )
    }

    /// The finding a sequence regression produces: `value` arrived behind the
    /// source's high-water mark and fills no known hole.
    ///
    /// A counter reset or a misrouted file. Reported under V18 because what it
    /// threatens is the same: until the source's sequence is understood, holes
    /// after it go unseen.
    #[must_use]
    pub fn sequence_regressed(source: &impl fmt::Display, value: u32, high_water: u32) -> Self {
        Self::batch(
            Rule::SequenceGap,
            format!("{source} sequence regressed: {value} behind {high_water}"),
        )
    }

    /// The finding a [`Reassembly`] produces, if it produces one.
    ///
    /// `None` for a complete run, an empty one, and an open run with no holes —
    /// none of those is a fault.
    #[must_use]
    pub fn reassembly(record_index: usize, assessment: &Reassembly) -> Option<Self> {
        match assessment {
            Reassembly::Orphan { reason, have } => Some(Self::new(
                Rule::OrphanPartial,
                record_index,
                match reason {
                    OrphanReason::FirstRecordNotFullyQualified => {
                        "first partial is reduced and has no basis to complete from".to_owned()
                    }
                    OrphanReason::InvalidSequence => {
                        format!("partial numbered 0 among {} present", have.len())
                    }
                },
            )),
            Reassembly::Incomplete {
                missing,
                missing_count,
                ..
            } if *missing_count > 0 => Some(Self::new(
                Rule::IncompleteReassembly,
                record_index,
                if *missing_count > missing.len() as u64 {
                    format!("{missing_count} missing partials, first {missing:?}")
                } else {
                    format!("missing partials {missing:?}")
                },
            )),
            _ => None,
        }
    }

    /// The same finding raised to blocking.
    #[must_use]
    pub fn escalated(mut self) -> Self {
        self.severity = Severity::Blocking;
        self
    }

    /// `true` when this finding stops the record being billed.
    #[must_use]
    pub const fn blocks_billing(&self) -> bool {
        matches!(self.severity, Severity::Blocking)
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.severity, self.rule)?;
        if let Some(index) = self.record_index {
            write!(f, " at record {index}")?;
        }
        if !self.detail.is_empty() {
            write!(f, ": {}", self.detail)?;
        }
        Ok(())
    }
}

// ── Configuration ────────────────────────────────────────────────────────────

/// What the stateless rules compare against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationConfig {
    /// The inclusion policy behind V10 and V11.
    pub lateness: LatenessPolicy,
    /// The highest sustained rate a bearer could plausibly carry, in octets per
    /// second, for V05.
    ///
    /// A ceiling, not a tariff: it exists to catch a corrupt counter, so it
    /// should sit well above any real bearer. The default of 10 GB/s is far
    /// above 5G peak rates and still catches a field read at the wrong offset.
    pub max_octets_per_second: u64,
    /// Rules to raise to [`Severity::Blocking`] beyond their default.
    ///
    /// Additive only. A rule's default severity can be tightened but never
    /// loosened, because the defaults encode "billing this invoices an amount
    /// that cannot be defended" — which is not a local preference.
    pub escalate: Vec<Rule>,
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            lateness: LatenessPolicy::tap3(),
            max_octets_per_second: 10_000_000_000,
            escalate: Vec::new(),
        }
    }
}

impl ValidationConfig {
    /// The severity this configuration gives a rule.
    #[must_use]
    pub fn severity_of(&self, rule: Rule) -> Severity {
        if self.escalate.contains(&rule) {
            Severity::Blocking
        } else {
            rule.default_severity()
        }
    }

    /// The same configuration with a rule raised to blocking.
    #[must_use]
    pub fn escalating(mut self, rule: Rule) -> Self {
        if !self.escalate.contains(&rule) {
            self.escalate.push(rule);
        }
        self
    }
}

// ── Report ───────────────────────────────────────────────────────────────────

/// Everything the rules found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    findings: Vec<Finding>,
}

impl ValidationReport {
    /// An empty report.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            findings: Vec::new(),
        }
    }

    /// Add a finding — used to fold in the stateful rules.
    pub fn push(&mut self, finding: Finding) {
        self.findings.push(finding);
    }

    /// Every finding, in the order the rules produced them.
    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// How many findings there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.findings.len()
    }

    /// `true` when nothing was found.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty()
    }

    /// `true` when any finding cites `rule`.
    #[must_use]
    pub fn has(&self, rule: Rule) -> bool {
        self.findings.iter().any(|f| f.rule == rule)
    }

    /// The severity this report gave `rule`, or `None` if it never fired.
    ///
    /// `None` rather than a default: "the rule did not fire" and "the rule
    /// fired as a warning" are different answers, and a caller that treats them
    /// alike will report a clean batch as noisy.
    ///
    /// The highest severity is returned when a rule fired more than once, so a
    /// single escalated instance is not masked by warnings beside it.
    #[must_use]
    pub fn severity_of(&self, rule: Rule) -> Option<Severity> {
        self.findings
            .iter()
            .filter(|f| f.rule == rule)
            .map(|f| f.severity)
            .max()
    }

    /// How many findings stop a record being billed.
    #[must_use]
    pub fn blocking_count(&self) -> usize {
        self.findings.iter().filter(|f| f.blocks_billing()).count()
    }

    /// `true` when anything found stops billing.
    #[must_use]
    pub fn blocks_billing(&self) -> bool {
        self.blocking_count() > 0
    }

    /// The findings against one record of the caller's slice.
    #[must_use]
    pub fn for_record(&self, index: usize) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|f| f.record_index == Some(index))
            .collect()
    }

    /// Re-apply a configuration's escalations to every finding held.
    ///
    /// [`validate_records`] does this before returning, but a report also
    /// gathers the **stateful** rules — a duplicate, a sequence hole, an
    /// unfinished run — which are found after it returns and would otherwise
    /// keep their default severity. A caller who escalates
    /// [`Rule::DuplicateRecord`] means it whichever stage found the duplicate.
    ///
    /// Idempotent, and never loosens: a configuration can only raise a rule.
    pub fn apply(&mut self, config: &ValidationConfig) {
        for finding in &mut self.findings {
            finding.severity = config.severity_of(finding.rule);
        }
    }

    /// `true` when the record at `index` may not be billed.
    #[must_use]
    pub fn record_blocked(&self, index: usize) -> bool {
        self.findings
            .iter()
            .any(|f| f.record_index == Some(index) && f.blocks_billing())
    }
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} finding(s), {} blocking",
            self.findings.len(),
            self.blocking_count()
        )
    }
}

impl FromIterator<Finding> for ValidationReport {
    fn from_iter<I: IntoIterator<Item = Finding>>(iter: I) -> Self {
        Self {
            findings: iter.into_iter().collect(),
        }
    }
}

// ── The engine ───────────────────────────────────────────────────────────────

/// Run every stateless rule over a slice of records.
///
/// `as_of` is supplied, never sampled, so replaying an archive reproduces the
/// verdicts it originally received rather than today's.
///
/// Findings index into `records` as passed, and the ordering rule is evaluated
/// against a sorted view so a shuffled input yields one finding rather than a
/// cascade.
#[must_use]
pub fn validate_records(
    records: &[UsageRecord],
    config: &ValidationConfig,
    as_of: OffsetDateTime,
) -> ValidationReport {
    validate_records_ordered_from(records, 0, config, as_of)
}

/// [`validate_records`], with V16 evaluated over `records[ordered_from..]`
/// only — the part of a batch that arrived in this batch's own order.
pub(crate) fn validate_records_ordered_from(
    records: &[UsageRecord],
    ordered_from: usize,
    config: &ValidationConfig,
    as_of: OffsetDateTime,
) -> ValidationReport {
    let mut report = ValidationReport::new();

    for (index, record) in records.iter().enumerate() {
        validate_one(record, index, config, as_of, &mut report);
    }
    check_ordering(records.get(ordered_from..).unwrap_or(&[]), &mut report);

    // Apply escalation once, at the end, so every path through the engine is
    // covered rather than each rule remembering to consult the config.
    report.apply(config);
    report
}

/// Every rule decidable from one record on its own.
fn validate_one(
    record: &UsageRecord,
    index: usize,
    config: &ValidationConfig,
    as_of: OffsetDateTime,
    report: &mut ValidationReport,
) {
    // V01 — an offset outside the assigned range describes no real place.
    for (label, timestamp) in [("opened", Some(record.opened)), ("closed", record.closed)] {
        if let Some(timestamp) = timestamp
            && !timestamp.offset().is_assigned()
        {
            report.push(Finding::new(
                Rule::UnassignedOffset,
                index,
                format!("{label} offset {}", timestamp.offset()),
            ));
        }
    }

    // V02 / V10 / V11 — where the record sits against the inclusion policy.
    match config.lateness.classify(record.opened.to_utc(), as_of) {
        Lateness::Future(by) => report.push(Finding::new(
            Rule::FutureTimestamp,
            index,
            format!("opens {by} ahead of the evaluation instant"),
        )),
        Lateness::BeyondDeadline(age) => report.push(Finding::new(
            Rule::BeyondDeadline,
            index,
            format!("{age} old"),
        )),
        Lateness::Late(age) => {
            report.push(Finding::new(Rule::LateArrival, index, format!("{age} old")))
        }
        Lateness::OnTime => {}
    }

    // V03 / V04 — the record's own timestamps and duration must be consistent.
    if let Some(closed) = record.closed {
        if closed.to_utc() < record.opened.to_utc() {
            report.push(Finding::new(
                Rule::ClosedBeforeOpened,
                index,
                format!("{} before {}", closed, record.opened),
            ));
        } else if let (Some(duration), Some(span)) = (record.duration, record.wall_clock_span())
            && duration > span
        {
            report.push(Finding::new(
                Rule::DurationExceedsSpan,
                index,
                format!("{duration} s billed over a {span} s span"),
            ));
        }
    }

    // V05 — a volume the elapsed time could not have carried.
    //
    // `billable_volume`, not the record-level field: a packet-data record
    // commonly states its volume only in its containers, and checking the
    // absent total would skip the plausibility ceiling on exactly the records
    // that carry the most.
    if let (Some(volume), Some(span)) = (record.billable_volume(), record.wall_clock_span())
        && !span.is_zero()
        && let Some(total) = volume.checked_total()
    {
        let ceiling = u128::from(span.get()) * u128::from(config.max_octets_per_second);
        if u128::from(total.get()) > ceiling {
            report.push(Finding::new(
                Rule::ImplausibleVolume,
                index,
                format!("{total} octets in {span} s"),
            ));
        }
    }

    // V15 — a record whose own volume cannot be summed: its containers, or
    // its two directions.
    if !record.containers.is_empty() && record.containers.checked_total().is_none() {
        report.push(Finding::new(
            Rule::VolumeOverflow,
            index,
            "container volumes overflow when summed",
        ));
    } else if let Some(volume) = record.billable_volume()
        && volume.checked_total().is_none()
    {
        report.push(Finding::new(
            Rule::VolumeOverflow,
            index,
            format!("{volume} overflows when summed"),
        ));
    }

    // V06 — nothing was measured.
    if record.is_zero_usage() {
        report.push(Finding::new(Rule::ZeroUsage, index, "no quantity recorded"));
    }

    // V07 — two statements of one fact that disagree.
    if let Some(false) = record.containers_agree() {
        let detail = match record.containers.discrepancy(record.volume) {
            Some((up, down)) => format!("record exceeds containers by {up}/{down} octets"),
            None => "record total and container sum differ".to_owned(),
        };
        report.push(Finding::new(Rule::ContainerSumMismatch, index, detail));
    }

    // V08 — nobody to charge.
    if let Some(reason) = record.party.unattributable_reason() {
        report.push(Finding::new(
            Rule::UnattributableParty,
            index,
            reason.to_string(),
        ));
    }

    // V09 — nothing stable to deduplicate on.
    if let Err(reason) = RecordKey::of(record) {
        report.push(Finding::not_keyable(index, reason));
    }

    // V12 — where the usage happened is unknown. A wireline access has no PLMN:
    // a fixed-line call or broadband session is located by its line, not by a
    // serving network, so the absence is not a finding there. The exemption is
    // limited to the domains a wireline access actually carries, so a mobile-only
    // record type mislabelled as wireline still raises it.
    let wireline = record.session.rat == Some(crate::session::RatType::Wireline)
        && matches!(
            record.record_type.service_domain(),
            ServiceDomain::Voice | ServiceDomain::Ims | ServiceDomain::FixedAccess
        );
    if record.serving_plmn.is_none()
        && record.record_type.service_domain().is_subscriber_bound()
        && !wireline
    {
        report.push(Finding::new(
            Rule::ServingNetworkAbsent,
            index,
            "no serving network on a subscriber-bound record",
        ));
    }

    // V13 — the quantity a service is measured by is missing.
    if !record.is_zero_usage() {
        let populated = match record.record_type.service_domain().primary_measure() {
            PrimaryMeasure::Duration => record.duration.is_some(),
            PrimaryMeasure::Volume => record.volume.is_some() || !record.containers.is_empty(),
            PrimaryMeasure::Events => record.events.is_some(),
        };
        if !populated {
            report.push(Finding::new(
                Rule::QuantityDoesNotMatchService,
                index,
                format!(
                    "{} is measured by {:?}",
                    record.record_type.service_domain(),
                    record.record_type.service_domain().primary_measure()
                ),
            ));
        }
    }

    // V21 — two sources in one record naming two countries. Not a tie to
    // break: filing the usage under either would pick a country no evidence
    // supports, and the country decides which regulator, tax authority and
    // retention rule applies.
    if let Some(Ambiguity::Conflicting { serving, reported }) = usage_country(record).ambiguity() {
        report.push(Finding::new(
            Rule::JurisdictionConflict,
            index,
            format!("serving network in {serving}, reported location in {reported}"),
        ));
    }

    // V14 — a check digit that does not verify is corrupt input, not a device.
    if let Some(iccid) = record.iccid
        && iccid.has_check_digit()
        && !iccid.luhn_valid()
    {
        report.push(Finding::new(
            Rule::CheckDigitFailure,
            index,
            format!("ICCID {iccid}"),
        ));
    }
}

/// V16 — the input is not in ascending instant order.
///
/// Reported **once** for the whole slice rather than per out-of-place record:
/// a shuffled input is one upstream fault, and a cascade of findings buries the
/// rules that matter.
fn check_ordering(records: &[UsageRecord], report: &mut ValidationReport) {
    let out_of_order = records
        .windows(2)
        .filter(|pair| {
            CdrTimestamp::cmp_instant(&pair[1].opened, &pair[0].opened) == std::cmp::Ordering::Less
        })
        .count();
    if out_of_order > 0 {
        report.push(Finding::batch(
            Rule::UnorderedSeries,
            format!(
                "{out_of_order} descending step(s) in {} records",
                records.len()
            ),
        ));
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{Finding, Rule, Severity, ValidationReport};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for Rule {
        /// The stable catalogue code, so a stored finding stays citable.
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.code())
        }
    }

    impl<'de> Deserialize<'de> for Rule {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = Rule;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a rule code such as \"V07\"")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<Rule, E> {
                    Rule::ALL
                        .iter()
                        .find(|r| r.code() == v)
                        .copied()
                        .ok_or_else(|| de::Error::custom(format!("unknown rule {v:?}")))
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    impl Serialize for Severity {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for Severity {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = Severity;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("\"WARNING\" or \"BLOCKING\"")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<Severity, E> {
                    match v {
                        "WARNING" => Ok(Severity::Warning),
                        "BLOCKING" => Ok(Severity::Blocking),
                        other => Err(de::Error::custom(format!("unknown severity {other:?}"))),
                    }
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "Finding")]
    struct FindingRepr {
        rule: Rule,
        severity: Severity,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        record_index: Option<usize>,
        detail: String,
    }

    impl Serialize for Finding {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            FindingRepr {
                rule: self.rule,
                severity: self.severity,
                record_index: self.record_index,
                detail: self.detail.clone(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Finding {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = FindingRepr::deserialize(deserializer)?;
            Ok(Finding {
                rule: r.rule,
                severity: r.severity,
                record_index: r.record_index,
                detail: r.detail,
            })
        }
    }

    impl Serialize for ValidationReport {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.findings().serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for ValidationReport {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Vec::<Finding>::deserialize(deserializer).map(ValidationReport::from_iter)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdrtime::Seconds;
    use crate::container::{ChangeCondition, ContainerSet, RatingGroup, ServiceDataContainer};
    use crate::correlation::{ChargingId, CorrelationKeys};
    use crate::party::{ChargedParty, SubscriberRef, UnattributableReason};
    use crate::record::{RecordType, SourceRef};
    use crate::volume::{Octets, VolumePair};
    use std::net::Ipv4Addr;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-07-28 12:00:00 UTC);

    fn keys() -> CorrelationKeys {
        CorrelationKeys::default()
            .with_charging_id(ChargingId::new(1))
            .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into())
    }

    fn call() -> UsageRecord {
        UsageRecord::new(
            RecordType::OriginatedCall,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new("msc-01".parse().unwrap(), 0),
        )
        .with_networks("262-02".parse().unwrap(), "262-02".parse().unwrap())
        .with_correlation(keys())
        .with_closed("2026-07-28T10:01:00+02:00".parse().unwrap())
        .with_duration(Seconds::new(55))
    }

    fn run(records: &[UsageRecord]) -> ValidationReport {
        validate_records(records, &ValidationConfig::default(), NOW)
    }

    /// A rule that never fired has no severity — not a defaulted one.
    #[test]
    fn a_rule_that_did_not_fire_has_no_severity() {
        let report = run(&[call()]);
        assert_eq!(report.severity_of(Rule::ZeroUsage), None);
        assert_eq!(report.severity_of(Rule::BeyondDeadline), None);
    }

    #[test]
    fn a_clean_record_produces_nothing() {
        let report = run(&[call()]);
        assert!(report.is_empty(), "{:?}", report.findings());
        assert!(!report.blocks_billing());
    }

    /// The rule that catches a duration nobody could have used.
    #[test]
    fn a_duration_longer_than_the_span_is_blocking() {
        let record = call()
            .with_closed("2026-07-28T10:00:10+02:00".parse().unwrap())
            .with_duration(Seconds::new(55));
        let report = run(&[record]);

        assert!(report.has(Rule::DurationExceedsSpan));
        assert_eq!(
            report.severity_of(Rule::DurationExceedsSpan),
            Some(Severity::Blocking)
        );
        assert!(report.record_blocked(0));
    }

    /// A record that closes before it opens is reported as such, and the
    /// duration rule does not also fire on the nonsense span.
    #[test]
    fn a_record_closing_before_it_opens_fires_one_rule_not_two() {
        let record = call().with_closed("2026-07-28T09:00:00+02:00".parse().unwrap());
        let report = run(&[record]);

        assert!(report.has(Rule::ClosedBeforeOpened));
        assert!(!report.has(Rule::DurationExceedsSpan));
    }

    #[test]
    fn the_lateness_policy_drives_three_rules() {
        // Well inside the 30-day window.
        assert!(!run(&[call()]).has(Rule::BeyondDeadline));

        let old = UsageRecord::new(
            RecordType::OriginatedCall,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-01-01T10:00:00+02:00".parse().unwrap(),
            SourceRef::new("msc-01".parse().unwrap(), 0),
        )
        .with_networks("262-02".parse().unwrap(), "262-02".parse().unwrap())
        .with_correlation(keys())
        .with_duration(Seconds::new(10));
        let report = run(&[old]);
        assert!(report.has(Rule::BeyondDeadline));
        assert_eq!(
            report.severity_of(Rule::BeyondDeadline),
            Some(Severity::Blocking)
        );

        let ahead = UsageRecord::new(
            RecordType::OriginatedCall,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-08-01T10:00:00+02:00".parse().unwrap(),
            SourceRef::new("msc-01".parse().unwrap(), 0),
        )
        .with_networks("262-02".parse().unwrap(), "262-02".parse().unwrap())
        .with_correlation(keys())
        .with_duration(Seconds::new(10));
        assert!(run(&[ahead]).has(Rule::FutureTimestamp));
    }

    /// Two statements of one fact that disagree, with the size of the gap.
    #[test]
    fn a_container_mismatch_reports_how_far_out_it_is() {
        let record = UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new("pgw-01".parse().unwrap(), 0),
        )
        .with_networks("262-02".parse().unwrap(), "262-02".parse().unwrap())
        .with_correlation(keys())
        .with_volume(VolumePair::new(Octets::new(150), Octets::new(900)))
        .with_containers(ContainerSet::from(vec![
            ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::RecordClosure)
                .with_volume(VolumePair::new(Octets::new(100), Octets::new(900))),
        ]));

        let report = run(&[record]);
        assert!(report.has(Rule::ContainerSumMismatch));
        let finding = report
            .findings()
            .iter()
            .find(|f| f.rule == Rule::ContainerSumMismatch)
            .unwrap();
        assert!(finding.detail.contains("50/0"), "{}", finding.detail);
        assert!(finding.blocks_billing());
    }

    #[test]
    fn an_unattributable_record_and_an_unkeyable_one_both_block() {
        let lost = UsageRecord::new(
            RecordType::OriginatedCall,
            ChargedParty::Unattributable(UnattributableReason::IdentityAbsent),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new("msc-01".parse().unwrap(), 0),
        )
        .with_networks("262-02".parse().unwrap(), "262-02".parse().unwrap())
        .with_duration(Seconds::new(10));

        let report = run(&[lost]);
        assert!(report.has(Rule::UnattributableParty));
        assert!(report.has(Rule::NotKeyable));
        assert!(report.record_blocked(0));
    }

    /// A volume the elapsed time could not have carried.
    #[test]
    fn an_implausible_volume_is_caught_but_does_not_block() {
        let record = UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new("pgw-01".parse().unwrap(), 0),
        )
        .with_networks("262-02".parse().unwrap(), "262-02".parse().unwrap())
        .with_correlation(keys())
        .with_closed("2026-07-28T10:00:01+02:00".parse().unwrap())
        .with_volume(VolumePair::new(Octets::new(u64::MAX / 2), Octets::ZERO));

        let report = run(&[record]);
        assert!(report.has(Rule::ImplausibleVolume));
        assert_eq!(
            report.severity_of(Rule::ImplausibleVolume),
            Some(Severity::Warning)
        );
        assert!(!report.record_blocked(0));
    }

    /// A shuffled input is one upstream fault, not a cascade.
    #[test]
    fn disorder_is_reported_once_for_the_whole_slice() {
        let mut early = call();
        early.opened = "2026-07-28T09:00:00+02:00".parse().unwrap();
        early.closed = Some("2026-07-28T09:01:00+02:00".parse().unwrap());

        let report = run(&[call(), early.clone(), call(), early]);
        let unordered: Vec<_> = report
            .findings()
            .iter()
            .filter(|f| f.rule == Rule::UnorderedSeries)
            .collect();
        assert_eq!(unordered.len(), 1);
        assert!(unordered[0].detail.contains('2'), "{}", unordered[0].detail);
        assert_eq!(unordered[0].record_index, None, "disorder is batch-level");
    }

    /// Findings index into the caller's slice, not into a sorted view.
    #[test]
    fn findings_point_into_the_callers_slice() {
        let broken = call()
            .with_closed("2026-07-28T10:00:01+02:00".parse().unwrap())
            .with_duration(Seconds::new(999));
        let report = run(&[call(), broken, call()]);

        let finding = report
            .findings()
            .iter()
            .find(|f| f.rule == Rule::DurationExceedsSpan)
            .unwrap();
        assert_eq!(finding.record_index, Some(1));
        assert_eq!(report.for_record(1).len(), 1);
        assert!(report.for_record(0).is_empty());
    }

    /// Severity can be tightened but never loosened.
    #[test]
    fn escalation_raises_a_warning_to_blocking() {
        let record = call().with_duration(Seconds::ZERO).with_events(0);
        assert_eq!(
            run(std::slice::from_ref(&record)).severity_of(Rule::ZeroUsage),
            Some(Severity::Warning)
        );

        let strict = ValidationConfig::default().escalating(Rule::ZeroUsage);
        let report = validate_records(&[record], &strict, NOW);
        assert_eq!(
            report.severity_of(Rule::ZeroUsage),
            Some(Severity::Blocking)
        );
        assert!(report.blocks_billing());
    }

    #[test]
    fn an_unassigned_offset_is_reported_for_either_timestamp() {
        // An offset the wire can hold but no place uses.
        let odd = CdrTimestamp::new(
            call().opened.local(),
            crate::cdrtime::QuarterHourOffset::from_encoded(79).unwrap(),
        )
        .unwrap();
        let mut record = call();
        record.opened = odd;
        let report = run(&[record]);
        assert!(report.has(Rule::UnassignedOffset));
        assert_eq!(
            report.severity_of(Rule::UnassignedOffset),
            Some(Severity::Blocking)
        );
    }

    /// Every rule has a distinct code, a description and a severity — a new
    /// variant cannot ship half-wired.
    #[test]
    fn the_catalogue_is_complete_and_distinct() {
        assert_eq!(Rule::ALL.len(), 21);
        let mut codes: Vec<&str> = Rule::ALL.iter().map(|r| r.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), Rule::ALL.len(), "codes must be distinct");

        for (position, rule) in Rule::ALL.iter().enumerate() {
            assert_eq!(rule.code(), format!("V{:02}", position + 1));
            assert!(!rule.describe().is_empty());
            assert!(rule.to_string().starts_with(rule.code()));
        }
    }

    /// Two sources naming two countries is a finding, because the country
    /// decides which regulator, tax authority and retention rule applies.
    #[test]
    fn a_country_disagreement_is_a_blocking_finding() {
        use crate::location::{Ncgi, Nci, UserLocation};
        use crate::session::SessionContext;

        let confused = call()
            .with_networks("214-07".parse().unwrap(), "262-02".parse().unwrap())
            .with_session(
                SessionContext::none().with_location(UserLocation::Ncgi(Ncgi::new(
                    "232-01".parse().unwrap(),
                    Nci::new(0x0_1234_5678).unwrap(),
                ))),
            );

        let report = validate_records(
            std::slice::from_ref(&confused),
            &ValidationConfig::default(),
            datetime!(2026-07-28 12:00 UTC),
        );

        let finding = report
            .findings()
            .iter()
            .find(|f| f.rule == Rule::JurisdictionConflict)
            .expect("V21 fires");
        assert_eq!(finding.severity, Severity::Blocking);
        assert!(finding.detail.contains("214"), "{}", finding.detail);
        assert!(finding.detail.contains("232"), "{}", finding.detail);

        // Agreeing sources produce no such finding.
        let agreeing = call()
            .with_networks("214-07".parse().unwrap(), "262-02".parse().unwrap())
            .with_session(
                SessionContext::none().with_location(UserLocation::Ncgi(Ncgi::new(
                    "214-07".parse().unwrap(),
                    Nci::new(0x0_1234_5678).unwrap(),
                ))),
            );
        let clean = validate_records(
            std::slice::from_ref(&agreeing),
            &ValidationConfig::default(),
            datetime!(2026-07-28 12:00 UTC),
        );
        assert_eq!(clean.severity_of(Rule::JurisdictionConflict), None);
    }

    #[test]
    fn stateful_outcomes_join_the_same_catalogue() {
        let key = RecordKey::of(&call()).unwrap();
        let duplicate = Finding::duplicate(3, &key);
        assert_eq!(duplicate.rule, Rule::DuplicateRecord);
        assert_eq!(duplicate.record_index, Some(3));

        let gap = Finding::sequence_gap(&"pgw-01", &[12, 13], 2);
        assert_eq!(gap.rule, Rule::SequenceGap);
        assert_eq!(gap.record_index, None);
        // A warning, not a block: the records of that source are held, which is
        // the stronger consequence, and blocking the batch as well would stop
        // every other source too.
        assert!(!gap.blocks_billing());
        assert!(gap.escalated().blocks_billing());

        // The count is exact even where the list is not.
        let truncated = Finding::sequence_gap(&"pgw-01", &[12, 13], 4_000);
        assert!(truncated.detail.contains("4000"));

        let complete = Reassembly::Complete { count: 3 };
        assert_eq!(Finding::reassembly(0, &complete), None);

        let orphan = Reassembly::Orphan {
            reason: OrphanReason::FirstRecordNotFullyQualified,
            have: vec![1],
        };
        let finding = Finding::reassembly(0, &orphan).unwrap();
        assert_eq!(finding.rule, Rule::OrphanPartial);
        assert!(finding.blocks_billing());
    }

    /// An open run with no holes is not a fault, and must not produce a finding.
    #[test]
    fn an_open_partial_run_is_not_a_finding() {
        let open = Reassembly::Incomplete {
            have: vec![1, 2],
            missing: vec![],
            missing_count: 0,
            terminated: false,
        };
        assert_eq!(Finding::reassembly(0, &open), None);

        let holed = Reassembly::Incomplete {
            have: vec![1, 3],
            missing: vec![2],
            missing_count: 1,
            terminated: true,
        };
        assert!(Finding::reassembly(0, &holed).is_some());
    }
}
