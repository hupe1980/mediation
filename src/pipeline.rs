//! One pass over a batch: the pieces of this crate, wired in a fixed order.
//!
//! Everything else here is a piece. This module is the assembly, and it exists
//! because the assembly is itself a decision that is easy to get wrong:
//! deduplicating after rejecting means a retransmission of a bad record is
//! rejected twice rather than recognised once; validating before checking
//! attributability means an unattributable record is reported as malformed;
//! holding a partial run after counting it billable double-bills the session
//! when its successor arrives.
//!
//! [`process`] fixes that order, documents it, and proves the result balances.
//!
//! # The state is yours
//!
//! Deduplication and gap detection are inherently stateful — you cannot
//! recognise a retransmission without remembering, and you cannot see a hole
//! without knowing what came before. So [`ProcessState`] is **caller-owned and
//! passed in**, which keeps [`process`] a pure mapping from
//! `(batch, config, as_of, state)` to `(outcome, state′)`.
//!
//! That is what makes a replay reproducible: hand it the same four things and
//! you get the same answer, in 2026 or in 2031. Nothing here reads a clock, so
//! `as_of` is a parameter and an archived batch reprocessed years later yields
//! the verdicts it originally did rather than the verdicts of today.
//!
//! ```rust
//! use mediation::{Batch, DedupWindow, ProcessConfig, ProcessState, process};
//! use time::{Duration, macros::datetime};
//! # use mediation::{ChargedParty, ChargingId, CorrelationKeys, Octets, RecordType,
//! #                 SourceRef, SubscriberRef, UsageRecord, VolumePair};
//! # use std::net::Ipv4Addr;
//! # fn session(id: u32) -> UsageRecord {
//! #     UsageRecord::new(
//! #         RecordType::PduSession,
//! #         ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
//! #         "2026-07-28T10:00:00+02:00".parse().unwrap(),
//! #         SourceRef::new("pgw-01".parse().unwrap(), 0).with_file_sequence(1),
//! #     )
//! #     .with_closed("2026-07-28T10:30:00+02:00".parse().unwrap())
//! #     .with_volume(VolumePair::new(Octets::new(1_000), Octets::new(4_000)))
//! #     .with_correlation(CorrelationKeys::default()
//! #         .with_charging_id(ChargingId::new(id))
//! #         .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()))
//! # }
//! let records: Vec<UsageRecord> = (1..=5).map(session).collect();
//!
//! let config = ProcessConfig::default();
//! let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
//!
//! let outcome = process(
//!     &Batch::new(&records),
//!     &config,
//!     datetime!(2026-07-28 12:00:00 UTC),
//!     &mut state,
//! );
//!
//! assert!(outcome.reconciliation().balances());
//! assert_eq!(outcome.reconciliation().released(), 5);
//! assert!(!outcome.quality().blocks_release());
//! ```
//!
//! # What it does not do
//!
//! [`process`] does not **correlate**, because correlation changes the
//! cardinality of the batch: two records describing one bearer become one
//! usage, and the conservation law counts records. Joining legs is
//! [`correlation::correlate`](crate::correlation::correlate), run before or after
//! this pass depending on whether your feed carries both sides in one file, and
//! its unmatched records are the evidence of leakage this crate refuses to drop.
//!
//! It does not **rate**, **aggregate by tariff** or **round**. What it returns
//! is a disposition per record, the counts, and the totals of what was
//! released.

use std::collections::{BTreeMap, BTreeSet};

use time::OffsetDateTime;

use crate::aggregation::{AggregationError, ReleasedTotals, aggregate_released_with_carry};
use crate::dedup::{DedupDecision, DedupStore, DedupWindow, RecordKey};
use crate::disposition::{
    BatchReconciliation, Disposition, HoldReason, RejectReason, SuppressReason,
};
use crate::lateness::Lateness;
use crate::partial::{PartialRun, Reassembly};
use crate::quality::{QualityReport, score_dispositions};
use crate::record::{NodeId, UsageRecord};
use crate::sequence::{SequenceObservation, SequenceSpace, SequenceTracker};
use crate::validation::{
    Finding, Rule, ValidationConfig, ValidationReport, validate_records_ordered_from,
};

// ── Suppression ──────────────────────────────────────────────────────────────

/// Decides whether a record is understood and deliberately not billed.
///
/// Suppression is **policy**, not a fault: a test SIM, a zero-rated emergency
/// call, traffic netted off between an operator's own networks. Which records
/// those are is a commercial and operational fact this crate does not hold —
/// the same stance it takes on E.212 widths, portability and EEA membership.
///
/// Returning `None` means *bill it if nothing else objects*.
pub trait Suppression {
    /// Why this record should not be billed, or `None` to let it through.
    fn suppress(&self, record: &UsageRecord) -> Option<SuppressReason>;
}

/// Suppress nothing.
///
/// The default, and the honest one: a crate that guessed which identities were
/// test SIMs would suppress real revenue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NoSuppression;

impl Suppression for NoSuppression {
    fn suppress(&self, _record: &UsageRecord) -> Option<SuppressReason> {
        None
    }
}

impl<F> Suppression for F
where
    F: Fn(&UsageRecord) -> Option<SuppressReason>,
{
    fn suppress(&self, record: &UsageRecord) -> Option<SuppressReason> {
        self(record)
    }
}

// ── Configuration ────────────────────────────────────────────────────────────

/// What [`process`] needs to know that is not in the records.
#[derive(Debug, Clone, Default)]
pub struct ProcessConfig {
    /// The rule catalogue's configuration — the inclusion policy, the
    /// plausibility ceiling, and any rules escalated to blocking.
    pub validation: ValidationConfig,
    /// Whether a record that measured nothing is suppressed rather than billed.
    ///
    /// Off by default. A zero-usage record is a finding either way
    /// ([`Rule::ZeroUsage`]), and whether it is *also* suppressed depends on
    /// what produces it in your network: a normal session-establishment record
    /// that carried no traffic, or a counter that failed to be read.
    pub suppress_zero_usage: bool,
}

impl ProcessConfig {
    /// The default configuration with zero-usage suppression turned on.
    #[must_use]
    pub fn suppressing_zero_usage(mut self) -> Self {
        self.suppress_zero_usage = true;
        self
    }
}

// ── The batch ────────────────────────────────────────────────────────────────

/// The records of one batch, and how many of them an earlier batch held.
///
/// A [`Disposition::Held`] record comes back. If the batch that re-presents it
/// counts it as newly ingested, the two batches together report it twice; if
/// neither counts it, it vanishes. Both are the conservation law failing at a
/// boundary, which is exactly where nobody looks — so a re-presented record is
/// counted through
/// [`observe_carried`](BatchReconciliation::observe_carried) instead.
///
/// The carried records come **first** in the slice, so there is one index space
/// over the whole batch and a [`Finding::record_index`] means one thing.
///
/// A carried record is not **observed** by deduplication — that would
/// recognise the hold coming back as a duplicate of itself — but its key is
/// **refreshed** to this batch's `as_of`, and put back if eviction dropped it
/// while the record waited. A genuine retransmission arriving while the record
/// is held, however long it is held, is still caught.
#[derive(Debug, Clone, Copy)]
pub struct Batch<'a> {
    records: &'a [UsageRecord],
    carried_in: usize,
}

impl<'a> Batch<'a> {
    /// A batch of newly arrived records.
    #[must_use]
    pub const fn new(records: &'a [UsageRecord]) -> Self {
        Self {
            records,
            carried_in: 0,
        }
    }

    /// A batch whose first `carried_in` records were held by an earlier batch.
    ///
    /// `None` when `carried_in` exceeds the slice, which would claim a carry
    /// the batch does not contain.
    #[must_use]
    pub const fn with_carry(records: &'a [UsageRecord], carried_in: usize) -> Option<Self> {
        if carried_in > records.len() {
            None
        } else {
            Some(Self {
                records,
                carried_in,
            })
        }
    }

    /// Every record, carried-in ones first.
    #[must_use]
    pub const fn records(&self) -> &'a [UsageRecord] {
        self.records
    }

    /// How many of the records an earlier batch held.
    #[must_use]
    pub const fn carried_in(&self) -> usize {
        self.carried_in
    }

    /// How many records arrived for the first time.
    #[must_use]
    pub const fn ingested(&self) -> usize {
        self.records.len() - self.carried_in
    }

    /// `true` when the record at `index` was held by an earlier batch.
    #[must_use]
    pub const fn is_carried(&self, index: usize) -> bool {
        index < self.carried_in
    }
}

// ── State ────────────────────────────────────────────────────────────────────

/// What must be remembered between batches.
///
/// Two things, and only two: which record keys have been seen, and how far each
/// source's file sequence has got. Both are the caller's to hold, persist and
/// restore — the crate performs no I/O, so a store that survives a restart is
/// something you serialise, not something it opens.
///
/// # It really does round-trip
///
/// These two are the crate's load-bearing correctness controls, and a restart
/// that forgot them would re-bill a window of retransmissions and declare every
/// source complete. So the whole of the state is reachable and rebuildable —
/// [`DedupStore::iter`] / [`DedupStore::restore`],
/// [`SequenceTracker::snapshot`] / [`SequenceTracker::restore`],
/// [`trackers`](Self::trackers) / [`with_tracker`](Self::with_tracker) — and
/// with the `serde` feature the whole `ProcessState` serialises in one call.
///
/// ```rust
/// # #[cfg(feature = "serde")] {
/// use mediation::{DedupWindow, ProcessState};
/// use time::Duration;
///
/// let state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
///
/// let saved = serde_json::to_string(&state).unwrap();
/// let restored: ProcessState = serde_json::from_str(&saved).unwrap();
/// assert_eq!(restored.dedup().len(), state.dedup().len());
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct ProcessState {
    dedup: DedupStore,
    space: SequenceSpace,
    sequences: BTreeMap<NodeId, SequenceTracker>,
}

impl ProcessState {
    /// State that has seen nothing, remembering keys for `window`.
    ///
    /// File sequences are tracked in [`SequenceSpace::U32`], the right space
    /// for a 3GPP file counter whose operator has not declared a narrower one.
    /// Use [`in_space`](Self::in_space) for a TAP relationship, whose numbers
    /// wrap at 99 999.
    #[must_use]
    pub fn new(window: DedupWindow) -> Self {
        Self::in_space(window, SequenceSpace::U32)
    }

    /// State that tracks file sequences in a given space.
    #[must_use]
    pub fn in_space(window: DedupWindow, space: SequenceSpace) -> Self {
        Self {
            dedup: DedupStore::new(window),
            space,
            sequences: BTreeMap::new(),
        }
    }

    /// The deduplication store, for eviction and inspection.
    #[must_use]
    pub const fn dedup(&self) -> &DedupStore {
        &self.dedup
    }

    /// The deduplication store, mutably — [`evict`](DedupStore::evict) is the
    /// caller's to schedule.
    pub const fn dedup_mut(&mut self) -> &mut DedupStore {
        &mut self.dedup
    }

    /// The sequence tracker for one source, if that source has been seen.
    #[must_use]
    pub fn tracker(&self, node: &NodeId) -> Option<&SequenceTracker> {
        self.sequences.get(node)
    }

    /// Every source being tracked, with its tracker.
    pub fn trackers(&self) -> impl Iterator<Item = (&NodeId, &SequenceTracker)> {
        self.sequences.iter()
    }

    /// The same state with one source's tracker installed.
    ///
    /// The restoring counterpart of [`trackers`](Self::trackers). `None` when
    /// the tracker follows a different [`SequenceSpace`] than this state does:
    /// a TAP tracker restored into a state that counts 32-bit file numbers
    /// would read every wrap as a four-billion-file gap.
    #[must_use]
    pub fn with_tracker(mut self, node: NodeId, tracker: SequenceTracker) -> Option<Self> {
        if tracker.space() != self.space {
            return None;
        }
        self.sequences.insert(node, tracker);
        Some(self)
    }

    /// The same state with a deduplication store installed.
    #[must_use]
    pub fn with_dedup(mut self, dedup: DedupStore) -> Self {
        self.dedup = dedup;
        self
    }

    /// The sequence space file counters are tracked in.
    #[must_use]
    pub const fn space(&self) -> SequenceSpace {
        self.space
    }

    /// Every source that still has an unfilled hole, with what it is missing.
    ///
    /// The operational view of completeness: the sources whose records this
    /// pass will hold, and the resend requests that would unblock them.
    pub fn incomplete_sources(&self) -> impl Iterator<Item = (&NodeId, &SequenceTracker)> {
        self.sequences
            .iter()
            .filter(|(_, tracker)| !tracker.is_complete())
    }

    /// `true` when no source has an unfilled hole.
    ///
    /// The completeness input to the release gate. A source with an unexplained
    /// hole has **unknown** completeness, and billing over unknown completeness
    /// invoices an amount nobody can prove is right.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.sequences.values().all(SequenceTracker::is_complete)
    }
}

// ── The outcome ──────────────────────────────────────────────────────────────

/// A hole in one source's file sequence.
///
/// Carries the identities rather than a count, because the operational
/// response is a resend request for exactly these files — and the exact count
/// beside them, because the list is bounded and the count is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceGap {
    /// The source whose sequence has a hole.
    pub node: NodeId,
    /// The lowest file sequence numbers that have not arrived, ascending.
    ///
    /// Bounded by [`SequenceTracker::DEFAULT_MAX_LISTED`]. Compare it against
    /// [`missing_count`](Self::missing_count) to see whether it was truncated.
    pub missing: Vec<u32>,
    /// Exactly how many files have not arrived. Never truncated.
    pub missing_count: u64,
}

impl SequenceGap {
    /// `true` when more files are missing than [`missing`](Self::missing) names.
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        self.missing_count > self.missing.len() as u64
    }
}

/// What one pass over a batch concluded.
#[derive(Debug, Clone)]
pub struct ProcessOutcome {
    dispositions: Vec<Disposition>,
    carried_in: usize,
    reconciliation: BatchReconciliation,
    findings: ValidationReport,
    quality: QualityReport,
    totals: Result<ReleasedTotals, AggregationError>,
    gaps: Vec<SequenceGap>,
}

impl ProcessOutcome {
    /// One disposition per record, in the batch's own order.
    #[must_use]
    pub fn dispositions(&self) -> &[Disposition] {
        &self.dispositions
    }

    /// The dispositions of the records an earlier batch had held.
    #[must_use]
    pub fn carried_dispositions(&self) -> &[Disposition] {
        &self.dispositions[..self.carried_in]
    }

    /// The dispositions of the records that arrived for the first time.
    #[must_use]
    pub fn ingested_dispositions(&self) -> &[Disposition] {
        &self.dispositions[self.carried_in..]
    }

    /// Where every record went.
    ///
    /// Balances by construction: one record, one increment, one outcome.
    #[must_use]
    pub const fn reconciliation(&self) -> &BatchReconciliation {
        &self.reconciliation
    }

    /// Every rule that fired, stateful ones included.
    ///
    /// Duplicates, sequence gaps and incomplete partial runs join the same
    /// numbered catalogue as the stateless rules, so a report has one
    /// vocabulary rather than four.
    #[must_use]
    pub const fn findings(&self) -> &ValidationReport {
        &self.findings
    }

    /// Whether the batch may be invoiced, and why not where it may not.
    #[must_use]
    pub const fn quality(&self) -> &QualityReport {
        &self.quality
    }

    /// What the released records add up to, with the counts that justify it.
    ///
    /// # Errors
    ///
    /// [`AggregationError`] when a released record disagrees with its own
    /// container breakdown, or the total overflows. Both are faults in the
    /// records rather than in the pass, and both name the record.
    pub fn totals(&self) -> Result<ReleasedTotals, AggregationError> {
        self.totals
    }

    /// The holes this pass opened in any source's file sequence.
    #[must_use]
    pub fn sequence_gaps(&self) -> &[SequenceGap] {
        &self.gaps
    }

    /// `true` when the batch may be released downstream.
    #[must_use]
    pub const fn is_releasable(&self) -> bool {
        !self.quality.blocks_release()
    }
}

// ── The pass ─────────────────────────────────────────────────────────────────

/// Run a batch through the layer, in one pass, and say what became of it.
///
/// # The order, and why it is this one
///
/// Each record reaches the first outcome that applies:
///
/// | # | Test | Outcome |
/// |---|---|---|
/// | 1 | no natural key | [`Rejected`](RejectReason::NotKeyable) |
/// | 2 | key already billed, held or suppressed inside the window | [`Duplicate`](Disposition::Duplicate) |
/// | 3 | no charged party | [`Rejected`](RejectReason::Unattributable) |
/// | 4 | timestamped ahead of `as_of` | [`Rejected`](RejectReason::FutureTimestamp) |
/// | 5 | past the inclusion deadline | [`Rejected`](RejectReason::BeyondDeadline) |
/// | 6 | a blocking rule fired | [`Rejected`](RejectReason::Malformed) |
/// | 7 | its run of partials can never be reassembled (V19) | [`Rejected`](RejectReason::Malformed) |
/// | 8 | its run of partials is not finished, first record included (V20) | [`Held`](HoldReason::AwaitingPartial) |
/// | 9 | the suppression policy says so | [`Suppressed`](Disposition::Suppressed) |
/// | 10 | its source has an unfilled sequence hole | [`Held`](HoldReason::SequenceGap) |
/// | 11 | otherwise | [`Billable`](Disposition::Billable) |
///
/// Three placements are load-bearing.
///
/// **Keyability before everything.** A record with no natural key cannot be
/// deduplicated, so billing it risks charging for it twice — the more expensive
/// of the two errors — and it is rejected before any test that might have let
/// it through.
///
/// **Deduplication before judgement.** A malformed retransmission of a record
/// already billed or held is a retransmission, not a second fault. Keys are
/// stamped with `as_of` — the arrival — so a late record is remembered for the
/// whole window after it arrived. A record **rejected** in this batch leaves no
/// key behind: it billed nothing, so a corrected resend, in this batch or a
/// later one, is judged on its own merits rather than discarded as a duplicate.
///
/// **Completeness is per source, not per batch — and it is the last test.** A
/// gateway with a lost file cannot prove what it is missing, so its records are
/// held; but only *its* records, and only the ones that were going to be
/// billed. Blocking the batch instead would stop an operator's whole billing run
/// on one node's hole while every other node's completeness was provable, and
/// holding a record the suppression policy had already ruled out would
/// re-present a foregone conclusion in every batch until the file arrived.
/// Because nothing from an incomplete source is ever *released*, what the batch
/// does release is complete by construction, and the release gate says so.
///
/// Each source's file sequences are observed in serial order, not in the order
/// the batch lists them, so shuffling a batch changes neither its holes nor its
/// dispositions. A sequence that moves backwards raises V18 as a regression.
///
/// Steps 7 and 8 apply only where the record's key names a **session** —
/// see [`RecordKey::session_scope`]. A partial keyed on a recording entity and
/// a serial number cannot be gathered into a run at all, so it passes through
/// rather than being judged against a run of one.
///
/// # Determinism
///
/// Nothing here reads a clock. `as_of` is the evaluation instant, so an
/// archived batch reprocessed years later yields the verdicts it originally did
/// rather than the verdicts of today — which is what a disputed billing cycle
/// needs.
#[must_use]
pub fn process(
    batch: &Batch<'_>,
    config: &ProcessConfig,
    as_of: OffsetDateTime,
    state: &mut ProcessState,
) -> ProcessOutcome {
    process_with(batch, config, as_of, state, &NoSuppression)
}

/// [`process`], with a suppression policy.
///
/// Separate rather than a field on [`ProcessConfig`] so the config stays a
/// plain, cloneable, comparable value — a policy is a closure or a type of
/// yours, and burying one in a config makes the config neither.
#[must_use]
pub fn process_with(
    batch: &Batch<'_>,
    config: &ProcessConfig,
    as_of: OffsetDateTime,
    state: &mut ProcessState,
    suppression: &impl Suppression,
) -> ProcessOutcome {
    let records = batch.records();

    // Stateless rules first, over the whole batch: the blocking findings decide
    // step 6. V16, the ordering rule, is a property of the series as it
    // arrived, so it reads only the ingested records — carried ones come first
    // by construction and say nothing about the sender's order.
    let mut findings =
        validate_records_ordered_from(records, batch.carried_in(), &config.validation, as_of);
    let stateless = findings.clone();

    // Completeness, per source. A file sequence is scoped to the node that
    // produced the file, and sharing one tracker across nodes manufactures gaps
    // that do not exist. Only the ingested records: a carried record's file
    // was observed in the batch that first saw it.
    let (gaps, regressions) =
        observe_sequences(&records[batch.carried_in()..], state, &mut findings);

    // Which of this batch's own sources cannot yet prove they are complete.
    // Computed after `observe_sequences`, so a hole this batch opened counts.
    let incomplete = incomplete_sources(records, state);

    // Partial runs, gathered by session. A run is assessed once and every
    // member of it takes the same verdict — a run is complete or it is not, and
    // holding half of one would leave the session partly billed.
    let runs = assess_runs(records);

    let mut dispositions = Vec::with_capacity(records.len());
    let mut reconciliation = BatchReconciliation::EMPTY;

    for (index, record) in records.iter().enumerate() {
        let carried = batch.is_carried(index);
        let disposition = disposition_for(
            index,
            record,
            carried,
            config,
            as_of,
            state,
            &stateless,
            &runs,
            &incomplete,
            suppression,
            &mut findings,
        );
        if carried {
            reconciliation.observe_carried(&disposition);
        } else {
            reconciliation.observe(&disposition);
        }
        dispositions.push(disposition);
    }

    // A file behind the high-water mark is a regression only if it brought
    // something new. A file whose every record is a duplicate is a resend.
    for (node, value, high_water) in regressions {
        let brought_something = records
            .iter()
            .zip(&dispositions)
            .skip(batch.carried_in())
            .any(|(record, disposition)| {
                record.provenance.node_id == node
                    && record.provenance.file_sequence == Some(value)
                    && !matches!(disposition, Disposition::Duplicate(_))
            });
        if brought_something {
            findings.push(Finding::sequence_regressed(&node, value, high_water));
        }
    }

    // Escalation applies to every finding the pass produced, not only to the
    // ones `validate_records` raised. A caller who escalates `DuplicateRecord`
    // means it whichever stage found the duplicate, and a report where half the
    // catalogue ignores the configuration is a report nobody can act on.
    findings.apply(&config.validation);

    // Nothing released came from a source with an unfilled hole — every such
    // record is held above — so the batch's own completeness is provable. A
    // blocking finding on a record rejected or held here has been acted on.
    let quality = score_dispositions(&reconciliation, &dispositions, &findings, true);
    let totals = aggregate_released_with_carry(records, &dispositions, batch.carried_in());

    ProcessOutcome {
        dispositions,
        carried_in: batch.carried_in(),
        reconciliation,
        findings,
        quality,
        totals,
        gaps,
    }
}

/// The verdict for one record, in the documented order.
#[allow(clippy::too_many_arguments)]
fn disposition_for(
    index: usize,
    record: &UsageRecord,
    carried: bool,
    config: &ProcessConfig,
    as_of: OffsetDateTime,
    state: &mut ProcessState,
    stateless: &ValidationReport,
    runs: &BTreeMap<RecordKey, Reassembly>,
    incomplete: &BTreeSet<NodeId>,
    suppression: &impl Suppression,
    findings: &mut ValidationReport,
) -> Disposition {
    // 1 — a record that cannot be deduplicated must not be billed. Its V09
    // is already among the stateless findings.
    let key = match RecordKey::of(record) {
        Ok(key) => key,
        Err(reason) => return Disposition::Rejected(RejectReason::NotKeyable(reason)),
    };

    // 2 — a retransmission is a retransmission, whatever else is wrong with it.
    //
    // Stamped with `as_of`, the arrival, so a late record is remembered for
    // the whole window. A **carried** record is not observed — that would find
    // its own hold — but refreshed, so its key outlives the wait.
    if carried {
        state.dedup.refresh(key.clone(), as_of);
    } else if let DedupDecision::Duplicate { .. } = state.dedup.observe(key.clone(), as_of) {
        findings.push(Finding::duplicate(index, &key));
        return Disposition::Duplicate(key);
    }

    let disposition = judge(
        index,
        record,
        &key,
        config,
        as_of,
        stateless,
        runs,
        incomplete,
        suppression,
        findings,
    );

    // A rejected record billed nothing, so its key must not turn a corrected
    // resend into a duplicate.
    if matches!(disposition, Disposition::Rejected(_)) {
        state.dedup.remove(&key);
    }
    disposition
}

/// Steps 3 to 11 for a record that is keyed and not a duplicate.
#[allow(clippy::too_many_arguments)]
fn judge(
    index: usize,
    record: &UsageRecord,
    key: &RecordKey,
    config: &ProcessConfig,
    as_of: OffsetDateTime,
    stateless: &ValidationReport,
    runs: &BTreeMap<RecordKey, Reassembly>,
    incomplete: &BTreeSet<NodeId>,
    suppression: &impl Suppression,
    findings: &mut ValidationReport,
) -> Disposition {
    // 3 — nobody to charge.
    if let Some(reason) = record.party.unattributable_reason() {
        return Disposition::Rejected(RejectReason::Unattributable(reason));
    }

    // 4 and 5 — where the record sits against the inclusion policy.
    match config
        .validation
        .lateness
        .classify(record.opened.to_utc(), as_of)
    {
        Lateness::Future(_) => return Disposition::Rejected(RejectReason::FutureTimestamp),
        Lateness::BeyondDeadline(_) => {
            return Disposition::Rejected(RejectReason::BeyondDeadline);
        }
        Lateness::OnTime | Lateness::Late(_) => {}
    }

    // 6 — a blocking rule means billing this would invoice something that
    // cannot be defended.
    if stateless.record_blocked(index) {
        return Disposition::Rejected(RejectReason::Malformed);
    }

    // 7 and 8 — the run this record belongs to, if it belongs to one.
    //
    // A record whose key names no session above it cannot be gathered into a
    // run at all — see `RecordKey::session_scope`. Such a partial passes
    // through here untouched rather than being judged against a run of one.
    if record.partial.is_some()
        && let Some(scope) = key.session_scope()
        && let Some(assessment) = runs.get(&scope)
    {
        // V19 for a run no wait can complete, V20 for one with holes — its
        // first record included, which may still arrive. An open run with no
        // holes is held without a finding: nothing is wrong with it yet.
        match assessment {
            Reassembly::Orphan { .. } => {
                if let Some(finding) = Finding::reassembly(index, assessment) {
                    findings.push(finding);
                }
                return Disposition::Rejected(RejectReason::Malformed);
            }
            Reassembly::Incomplete { .. } => {
                if let Some(finding) = Finding::reassembly(index, assessment) {
                    findings.push(finding);
                }
                return Disposition::Held(HoldReason::AwaitingPartial);
            }
            Reassembly::Complete { .. } | Reassembly::Empty => {}
        }
    }

    // 9 — understood, and deliberately not billed.
    //
    // Before the completeness hold, deliberately. A record nobody was going to
    // bill does not need its source's completeness proved: holding it would
    // re-present a foregone conclusion in every batch until the missing file
    // arrives, and the answer would be the same each time.
    if let Some(reason) = suppression.suppress(record) {
        return Disposition::Suppressed(reason);
    }
    if config.suppress_zero_usage && record.is_zero_usage() {
        return Disposition::Suppressed(SuppressReason::ZeroUsage);
    }

    // 10 — this record would be billed, and its source cannot prove it is
    // complete. Held rather than rejected: the missing file may still arrive,
    // and `SequenceTracker::abandon_older_than` is how a caller decides it
    // never will.
    //
    // Scoped to the record's own source. Blocking the whole batch instead would
    // stop an operator's entire billing run on one gateway's lost file, while
    // every other gateway's completeness was perfectly provable.
    if incomplete.contains(&record.provenance.node_id) {
        return Disposition::Held(HoldReason::SequenceGap);
    }

    // 11 — nothing objected.
    Disposition::Billable
}

/// A file that arrived behind its source's high-water mark and filled no hole:
/// the source, the file sequence and the mark.
type Regression = (NodeId, u32, u32);

/// Feed each source's file sequence to its tracker, report what is missing, and
/// return the files that arrived behind the high-water mark.
fn observe_sequences(
    records: &[UsageRecord],
    state: &mut ProcessState,
    findings: &mut ValidationReport,
) -> (Vec<SequenceGap>, Vec<Regression>) {
    // One observation per (node, file), not per record: a file's sequence
    // number is a property of the file, and observing it once per record it
    // holds would report every record after the first as a duplicate arrival.
    // And in serial order, not batch order: listing file 3 before file 1 must
    // not hide the hole at 2.
    let mut by_node: BTreeMap<&NodeId, Vec<u32>> = BTreeMap::new();
    for record in records {
        if let Some(sequence) = record.provenance.file_sequence {
            by_node
                .entry(&record.provenance.node_id)
                .or_default()
                .push(sequence);
        }
    }

    let mut gaps: Vec<SequenceGap> = Vec::new();
    let mut regressions: Vec<Regression> = Vec::new();
    for (node, sequences) in by_node {
        let tracker = state
            .sequences
            .entry(node.clone())
            .or_insert_with(|| SequenceTracker::new(state.space));
        let observed = tracker.observe_all(sequences, SequenceTracker::DEFAULT_MAX_LISTED);
        let high_water = tracker.high_water();
        for (value, observation) in observed {
            match observation {
                SequenceObservation::Gap {
                    missing,
                    missing_count,
                } => {
                    findings.push(Finding::sequence_gap(node, &missing, missing_count));
                    gaps.push(SequenceGap {
                        node: node.clone(),
                        missing,
                        missing_count,
                    });
                }
                SequenceObservation::Unexpected => {
                    if let Some(high) = high_water {
                        regressions.push((node.clone(), value, high));
                    }
                }
                _ => {}
            }
        }
    }
    (gaps, regressions)
}

/// The sources this batch drew records from that still have an unfilled hole.
///
/// Scoped to the sources the batch actually used, not to every source the state
/// has ever tracked. A hole at the Frankfurt gateway says nothing about the
/// completeness of a batch that carries only Munich's records, and letting it
/// block that batch stops a whole operator's billing on one node's lost file.
fn incomplete_sources(records: &[UsageRecord], state: &ProcessState) -> BTreeSet<NodeId> {
    let mut found = BTreeSet::new();
    for record in records {
        let node = &record.provenance.node_id;
        if found.contains(node) {
            continue;
        }
        if state
            .sequences
            .get(node)
            .is_some_and(|tracker| !tracker.is_complete())
        {
            found.insert(node.clone());
        }
    }
    found
}

/// Gather every partial in the batch into the run it belongs to, and assess it.
fn assess_runs(records: &[UsageRecord]) -> BTreeMap<RecordKey, Reassembly> {
    let mut runs: BTreeMap<RecordKey, PartialRun> = BTreeMap::new();
    for record in records {
        let (Some(partial), Ok(key)) = (record.partial, RecordKey::of(record)) else {
            continue;
        };
        // No session identity, no run to gather it into.
        let Some(scope) = key.session_scope() else {
            continue;
        };
        runs.entry(scope).or_default().push(partial);
    }
    runs.into_iter()
        .map(|(scope, run)| {
            let assessment = run.assess();
            (scope, assessment)
        })
        .collect()
}

/// Raise the findings a run's assessment implies, for a caller that assembled
/// the runs themselves.
///
/// [`process`] does this internally; this is the same mapping exposed for a
/// caller who gathers runs across batches rather than within one.
#[must_use]
pub fn reassembly_finding(record_index: usize, assessment: &Reassembly) -> Option<Finding> {
    Finding::reassembly(record_index, assessment)
}

/// The rule a disposition corresponds to, where it corresponds to one.
///
/// Present so a caller mapping dispositions onto their own storage codes can
/// cite the catalogue rather than inventing a second vocabulary.
#[must_use]
pub const fn rule_for(disposition: &Disposition) -> Option<Rule> {
    match disposition {
        Disposition::Duplicate(_) => Some(Rule::DuplicateRecord),
        Disposition::Rejected(RejectReason::NotKeyable(_)) => Some(Rule::NotKeyable),
        Disposition::Rejected(RejectReason::Unattributable(_)) => Some(Rule::UnattributableParty),
        Disposition::Rejected(RejectReason::BeyondDeadline) => Some(Rule::BeyondDeadline),
        Disposition::Rejected(RejectReason::FutureTimestamp) => Some(Rule::FutureTimestamp),
        Disposition::Held(HoldReason::AwaitingPartial) => Some(Rule::IncompleteReassembly),
        Disposition::Held(HoldReason::SequenceGap) => Some(Rule::SequenceGap),
        _ => None,
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{ProcessState, SequenceGap};
    use crate::dedup::DedupStore;
    use crate::record::NodeId;
    use crate::sequence::{SequenceSpace, SequenceTracker};
    use serde::de;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    /// The whole of what must survive a restart, in one value.
    ///
    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "ProcessState")]
    struct StateRepr {
        dedup: DedupStore,
        space: SequenceSpace,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        sequences: BTreeMap<NodeId, SequenceTracker>,
    }

    impl Serialize for ProcessState {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            StateRepr {
                dedup: self.dedup().clone(),
                space: self.space(),
                sequences: self
                    .trackers()
                    .map(|(node, tracker)| (node.clone(), tracker.clone()))
                    .collect(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for ProcessState {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = StateRepr::deserialize(deserializer)?;
            let mut state =
                ProcessState::in_space(r.dedup.window(), r.space).with_dedup(r.dedup.clone());
            for (node, tracker) in r.sequences {
                state = state.with_tracker(node, tracker).ok_or_else(|| {
                    de::Error::custom("every tracker to follow the state's own sequence space")
                })?;
            }
            Ok(state)
        }
    }

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "SequenceGap")]
    struct Repr {
        node: NodeId,
        missing: Vec<u32>,
        missing_count: u64,
    }

    impl Serialize for SequenceGap {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            Repr {
                node: self.node.clone(),
                missing: self.missing.clone(),
                missing_count: self.missing_count,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for SequenceGap {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = Repr::deserialize(deserializer)?;
            Ok(SequenceGap {
                node: r.node,
                missing: r.missing,
                missing_count: r.missing_count,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::correlation::{ChargingId, CorrelationKeys};
    use crate::partial::{PartialFormat, PartialInfo, RecordClosureCause};
    use crate::party::{ChargedParty, SubscriberRef};
    use crate::record::{RecordType, SourceRef};
    use crate::volume::{Octets, VolumePair};
    use std::net::Ipv4Addr;
    use time::{Duration, macros::datetime};

    const NOW: OffsetDateTime = datetime!(2026-07-28 12:00:00 UTC);

    fn window() -> DedupWindow {
        DedupWindow::from_bound(Duration::days(7)).expect("positive")
    }

    fn session(charging_id: u32) -> UsageRecord {
        UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new("pgw-01".parse().unwrap(), 0).with_file_sequence(1),
        )
        .with_closed("2026-07-28T10:30:00+02:00".parse().unwrap())
        .with_volume(VolumePair::new(Octets::new(1_000), Octets::new(4_000)))
        .with_correlation(
            CorrelationKeys::default()
                .with_charging_id(ChargingId::new(charging_id))
                .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()),
        )
    }

    #[test]
    fn a_clean_batch_releases_and_balances() {
        let records: Vec<UsageRecord> = (1..=5).map(session).collect();
        let mut state = ProcessState::new(window());

        let outcome = process(
            &Batch::new(&records),
            &ProcessConfig::default(),
            NOW,
            &mut state,
        );

        assert_eq!(outcome.reconciliation().released(), 5);
        assert!(outcome.reconciliation().balances());
        assert!(outcome.is_releasable());
        assert_eq!(outcome.totals().unwrap().total().records(), 5);
    }

    /// A malformed retransmission of a billed record is a retransmission, not
    /// a second fault. A rejected copy leaves no key, so its resend is judged.
    #[test]
    fn deduplication_runs_before_judgement() {
        let good = session(1);
        let mut broken = session(1);
        broken.closed = Some("2026-07-28T09:00:00+02:00".parse().unwrap()); // closed before it opened

        let mut state = ProcessState::new(window());
        let records = vec![good, broken.clone()];
        let outcome = process(
            &Batch::new(&records),
            &ProcessConfig::default(),
            NOW,
            &mut state,
        );
        assert_eq!(outcome.reconciliation().released(), 1);
        assert_eq!(outcome.reconciliation().duplicate(), 1);
        assert!(outcome.reconciliation().balances());

        let mut state = ProcessState::new(window());
        let records = vec![broken.clone(), broken];
        let outcome = process(
            &Batch::new(&records),
            &ProcessConfig::default(),
            NOW,
            &mut state,
        );
        assert_eq!(outcome.reconciliation().rejected(), 2);
        assert!(state.dedup().is_empty(), "a rejected record leaves no key");
        assert!(outcome.reconciliation().balances());
    }

    /// A record with no natural key is rejected before any test that might have
    /// let it through: billing it risks charging for it twice.
    #[test]
    fn an_unkeyable_record_is_rejected_first() {
        let bare = UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new("pgw-01".parse().unwrap(), 0),
        );
        let records = vec![bare];

        let mut state = ProcessState::new(window());
        let outcome = process(
            &Batch::new(&records),
            &ProcessConfig::default(),
            NOW,
            &mut state,
        );

        assert_eq!(outcome.reconciliation().rejected(), 1);
        assert!(matches!(
            outcome.dispositions()[0],
            Disposition::Rejected(RejectReason::NotKeyable(_))
        ));
        assert!(outcome.findings().has(Rule::NotKeyable));
    }

    /// A run that is not finished is held, not billed — every member of it, so
    /// the session is never partly billed.
    #[test]
    fn an_unfinished_partial_run_is_held_in_full() {
        let mut first = session(7);
        first.partial = Some(PartialInfo::new(
            1,
            PartialFormat::FullyQualified,
            RecordClosureCause::VolumeLimit,
        ));
        first.correlation.record_sequence_number = Some(1);
        let mut second = session(7);
        second.partial = Some(PartialInfo::new(
            2,
            PartialFormat::Reduced,
            RecordClosureCause::VolumeLimit,
        ));
        second.correlation.record_sequence_number = Some(2);

        let records = vec![first, second];
        let mut state = ProcessState::new(window());
        let outcome = process(
            &Batch::new(&records),
            &ProcessConfig::default(),
            NOW,
            &mut state,
        );

        assert_eq!(outcome.reconciliation().held(), 2);
        assert_eq!(outcome.reconciliation().released(), 0);
        assert!(outcome.reconciliation().balances());
        assert!(
            !outcome.quality().is_final(),
            "a held record reopens a period"
        );
    }

    /// A hole in a source's file sequence is the only evidence of loss, and it
    /// blocks the release rather than grading it down.
    #[test]
    fn a_sequence_hole_holds_its_own_source() {
        let mut first = session(1);
        first.provenance = first.provenance.clone().with_file_sequence(1);
        let mut third = session(2);
        third.provenance = third.provenance.clone().with_file_sequence(3);

        let records = vec![first, third];
        let mut state = ProcessState::new(window());
        let outcome = process(
            &Batch::new(&records),
            &ProcessConfig::default(),
            NOW,
            &mut state,
        );

        assert_eq!(outcome.sequence_gaps().len(), 1);
        assert_eq!(outcome.sequence_gaps()[0].missing, vec![2]);
        assert_eq!(outcome.sequence_gaps()[0].missing_count, 1);
        assert!(outcome.findings().has(Rule::SequenceGap));

        // Both records come from the one source, so both wait. Nothing was
        // released, so there is nothing whose completeness is unprovable — the
        // hold is the enforcement, not the grade.
        assert_eq!(
            outcome.dispositions(),
            &[
                Disposition::Held(HoldReason::SequenceGap),
                Disposition::Held(HoldReason::SequenceGap),
            ]
        );
        assert_eq!(outcome.reconciliation().released(), 0);
        assert!(outcome.reconciliation().balances());
        assert!(!state.is_complete());
    }

    /// A caller who would rather the whole batch waited can say so, and the
    /// catalogue's own escalation is how they say it.
    #[test]
    fn a_sequence_hole_can_be_escalated_to_block_the_batch() {
        let mut first = session(1);
        first.provenance = first.provenance.clone().with_file_sequence(1);
        let mut third = session(2);
        third.provenance = third.provenance.clone().with_file_sequence(3);

        let records = vec![first, third];
        let mut state = ProcessState::new(window());
        let config = ProcessConfig {
            validation: ValidationConfig::default().escalating(Rule::SequenceGap),
            ..ProcessConfig::default()
        };
        let outcome = process(&Batch::new(&records), &config, NOW, &mut state);

        assert_eq!(
            outcome.findings().severity_of(Rule::SequenceGap),
            Some(crate::validation::Severity::Blocking),
            "escalation reaches the stateful rules too"
        );
        assert!(!outcome.is_releasable());
    }

    /// A carried record is counted as a carry, not as a new arrival — the one
    /// way the conservation law fails at a batch boundary.
    #[test]
    fn a_carried_record_is_not_counted_as_newly_ingested() {
        let records: Vec<UsageRecord> = (1..=3).map(session).collect();
        let batch = Batch::with_carry(&records, 1).unwrap();

        let mut state = ProcessState::new(window());
        let outcome = process(&batch, &ProcessConfig::default(), NOW, &mut state);

        assert_eq!(outcome.reconciliation().carried_in(), 1);
        assert_eq!(outcome.reconciliation().ingested(), 2);
        assert_eq!(outcome.carried_dispositions().len(), 1);
        assert_eq!(outcome.ingested_dispositions().len(), 2);
        assert!(outcome.reconciliation().balances());

        // A carry larger than the batch claims records the batch does not hold.
        assert!(Batch::with_carry(&records, 4).is_none());
    }

    /// Suppression is policy, and the crate holds none of it.
    #[test]
    fn a_suppression_policy_is_the_callers() {
        let records: Vec<UsageRecord> = (1..=3).map(session).collect();
        let mut state = ProcessState::new(window());

        let only_the_first = |record: &UsageRecord| {
            (record.correlation.charging_id == Some(ChargingId::new(1)))
                .then_some(SuppressReason::TestIdentity)
        };

        let outcome = process_with(
            &Batch::new(&records),
            &ProcessConfig::default(),
            NOW,
            &mut state,
            &only_the_first,
        );

        assert_eq!(outcome.reconciliation().suppressed(), 1);
        assert_eq!(outcome.reconciliation().released(), 2);
        assert!(outcome.reconciliation().balances());
    }

    /// The whole point: whatever the batch contains, every record reaches
    /// exactly one outcome.
    #[test]
    fn every_record_reaches_exactly_one_outcome() {
        let mut records: Vec<UsageRecord> = (1..=4).map(session).collect();
        records.push(session(1)); // a duplicate
        records.push(UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Unattributable(crate::party::UnattributableReason::IdentityAbsent),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new("pgw-01".parse().unwrap(), 9),
        )); // unkeyable and unattributable
        let mut ancient = session(9);
        ancient.opened = "2026-01-01T10:00:00+02:00".parse().unwrap();
        records.push(ancient);

        let mut state = ProcessState::new(window());
        let outcome = process(
            &Batch::new(&records),
            &ProcessConfig::default(),
            NOW,
            &mut state,
        );

        assert_eq!(outcome.dispositions().len(), records.len());
        assert_eq!(
            outcome.reconciliation().presented(),
            Some(records.len() as u64)
        );
        assert!(outcome.reconciliation().balances());
    }
}
