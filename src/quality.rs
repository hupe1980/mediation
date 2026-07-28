//! The release gate: may this batch be invoiced?
//!
//! One question, answered from three inputs — whether the batch balances,
//! whether its sources are complete, and what the rules found.
//!
//! ```rust
//! use mediation::{BatchReconciliation, Disposition, QualityGrade, ValidationReport, score_batch};
//!
//! let counts: BatchReconciliation = vec![Disposition::Billable; 100].into_iter().collect();
//! let report = score_batch(&counts, &ValidationReport::new(), true);
//!
//! assert_eq!(report.grade, QualityGrade::A);
//! assert!(!report.blocks_release());
//! ```
//!
//! # Why completeness is an input rather than a finding
//!
//! A source with an unfilled sequence hole has **unknown** completeness, and
//! that is categorically different from a known fault. Nothing in the batch is
//! wrong; what is wrong is that the batch cannot say what it is missing.
//! Releasing it invoices an amount nobody can prove is right, so it is a hard
//! gate rather than a score that enough good records could outvote.
//!
//! # Why a grade at all
//!
//! The gate is a boolean, but an operator needs to know whether a batch is
//! clean, merely noisy, or deteriorating. The grade is that signal, and it is
//! deliberately coarse: four values that fit in a dashboard column and mean the
//! same thing to everyone reading it.

use std::fmt;
use std::str::FromStr;

use crate::disposition::BatchReconciliation;
use crate::error::ParseError;
use crate::validation::ValidationReport;

/// How clean a batch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QualityGrade {
    /// Balanced, complete, nothing found. Release.
    A,
    /// Balanced and complete; a few warnings. Release, logged.
    B,
    /// Balanced and complete; many warnings, or records still held. Release
    /// with a reconciliation note.
    C,
    /// Unbalanced, incomplete, or something blocking. **Do not release.**
    F,
}

impl QualityGrade {
    /// Every grade, cleanest first.
    pub const ALL: &'static [Self] = &[Self::A, Self::B, Self::C, Self::F];

    /// `true` for the one grade that stops a release.
    #[must_use]
    pub const fn blocks_release(self) -> bool {
        matches!(self, Self::F)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::F => "F",
        }
    }
}

impl fmt::Display for QualityGrade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for QualityGrade {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "A" => Ok(Self::A),
            "B" => Ok(Self::B),
            "C" => Ok(Self::C),
            "F" => Ok(Self::F),
            _ => Err(ParseError::one_of("QualityGrade", s, &["A", "B", "C", "F"])),
        }
    }
}

/// Why a batch may not be released.
///
/// Each variant is independently sufficient, and all that apply are reported —
/// fixing one does not imply the batch is releasable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReleaseBlocker {
    /// The disposition counts do not satisfy the conservation law.
    Unbalanced,
    /// A source has an unfilled sequence hole, so completeness is unknown.
    IncompleteSources,
    /// At least one rule fired at blocking severity.
    BlockingFindings,
}

impl ReleaseBlocker {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[
        Self::Unbalanced,
        Self::IncompleteSources,
        Self::BlockingFindings,
    ];

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unbalanced => "UNBALANCED",
            Self::IncompleteSources => "INCOMPLETE_SOURCES",
            Self::BlockingFindings => "BLOCKING_FINDINGS",
        }
    }
}

impl fmt::Display for ReleaseBlocker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the gate decided, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityReport {
    /// How clean the batch is.
    pub grade: QualityGrade,
    /// Every reason the batch may not be released, in a stable order.
    pub blockers: Vec<ReleaseBlocker>,
    /// How many findings fired at blocking severity.
    pub blocking_findings: usize,
    /// How many findings fired at warning severity.
    pub warnings: usize,
    /// How many records are waiting for something that may still arrive.
    pub held: u64,
}

impl QualityReport {
    /// `true` when the batch may not be invoiced.
    ///
    /// The one call a release path needs to make.
    #[must_use]
    pub const fn blocks_release(&self) -> bool {
        !self.blockers.is_empty()
    }

    /// `true` when the batch is releasable **and** nothing is still in flight.
    ///
    /// The precondition for closing a period: a held record may still become
    /// billable, so a period closed while records are held will need reopening.
    #[must_use]
    pub const fn is_final(&self) -> bool {
        self.blockers.is_empty() && self.held == 0
    }
}

impl fmt::Display for QualityReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "grade {} ({} blocking, {} warning, {} held)",
            self.grade, self.blocking_findings, self.warnings, self.held
        )?;
        if !self.blockers.is_empty() {
            write!(f, " — blocked by ")?;
            for (position, blocker) in self.blockers.iter().enumerate() {
                if position > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{blocker}")?;
            }
        }
        Ok(())
    }
}

/// Grade a batch and decide whether it may be released.
///
/// `sources_complete` is the caller's answer to *"does every
/// [`crate::sequence::SequenceTracker`] feeding this batch report
/// [`is_complete`](crate::sequence::SequenceTracker::is_complete)?"* — the crate
/// cannot know how many sources a batch drew on, and guessing would be the one
/// mistake that lets an incomplete batch through.
///
/// The grade is derived, not configurable: the boundaries encode when an
/// operator should look, and a per-deployment threshold would make two teams'
/// "grade B" mean different things.
#[must_use]
pub fn score_batch(
    counts: &BatchReconciliation,
    findings: &ValidationReport,
    sources_complete: bool,
) -> QualityReport {
    let blocking_findings = findings.blocking_count();
    let warnings = findings.len() - blocking_findings;

    let mut blockers = Vec::new();
    if !counts.balances() {
        blockers.push(ReleaseBlocker::Unbalanced);
    }
    if !sources_complete {
        blockers.push(ReleaseBlocker::IncompleteSources);
    }
    if blocking_findings > 0 {
        blockers.push(ReleaseBlocker::BlockingFindings);
    }

    let grade = if blockers.is_empty() {
        grade_clean_batch(counts, warnings)
    } else {
        QualityGrade::F
    };

    QualityReport {
        grade,
        blockers,
        blocking_findings,
        warnings,
        held: counts.held(),
    }
}

/// The grade for a batch that is releasable, from how noisy it is.
///
/// Thresholds are proportions rather than counts, so a batch of a hundred and a
/// batch of a hundred million grade the same way for the same defect rate.
fn grade_clean_batch(counts: &BatchReconciliation, warnings: usize) -> QualityGrade {
    let ingested = counts.ingested();
    if ingested == 0 {
        // An empty batch is trivially clean. It is also not evidence of
        // anything, which the caller sees in `ingested`.
        return QualityGrade::A;
    }

    let warnings = warnings as u64;
    if warnings == 0 && counts.held() == 0 {
        return QualityGrade::A;
    }
    // Below one percent noise, and under a tenth held, is a working batch.
    if warnings * 100 <= ingested && counts.held() * 10 <= ingested {
        return QualityGrade::B;
    }
    QualityGrade::C
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{QualityGrade, QualityReport, ReleaseBlocker};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for QualityGrade {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for QualityGrade {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = QualityGrade;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a quality grade: \"A\", \"B\", \"C\" or \"F\"")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<QualityGrade, E> {
                    v.parse().map_err(de::Error::custom)
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    impl Serialize for ReleaseBlocker {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for ReleaseBlocker {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = ReleaseBlocker;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a release blocker code")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<ReleaseBlocker, E> {
                    ReleaseBlocker::ALL
                        .iter()
                        .find(|b| b.as_str() == v)
                        .copied()
                        .ok_or_else(|| de::Error::custom(format!("unknown blocker {v:?}")))
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "QualityReport")]
    struct Repr {
        grade: QualityGrade,
        blockers: Vec<ReleaseBlocker>,
        blocking_findings: usize,
        warnings: usize,
        held: u64,
    }

    impl Serialize for QualityReport {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            Repr {
                grade: self.grade,
                blockers: self.blockers.clone(),
                blocking_findings: self.blocking_findings,
                warnings: self.warnings,
                held: self.held,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for QualityReport {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = Repr::deserialize(deserializer)?;
            Ok(QualityReport {
                grade: r.grade,
                blockers: r.blockers,
                blocking_findings: r.blocking_findings,
                warnings: r.warnings,
                held: r.held,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dedup::{NotKeyable, RecordKey};
    use crate::disposition::{Disposition, HoldReason, RejectReason, SuppressReason};
    use crate::validation::{Finding, Rule};

    fn counts(dispositions: Vec<Disposition>) -> BatchReconciliation {
        dispositions.into_iter().collect()
    }

    fn clean(n: usize) -> BatchReconciliation {
        counts(vec![Disposition::Billable; n])
    }

    fn warnings(n: usize) -> ValidationReport {
        (0..n)
            .map(|i| Finding::new(Rule::ZeroUsage, i, "no quantity"))
            .collect()
    }

    #[test]
    fn a_clean_complete_batch_grades_a_and_releases() {
        let report = score_batch(&clean(100), &ValidationReport::new(), true);
        assert_eq!(report.grade, QualityGrade::A);
        assert!(!report.blocks_release());
        assert!(report.is_final());
        assert!(report.blockers.is_empty());
    }

    /// The hard gate: a source with an unfilled hole has unknown completeness,
    /// which no number of good records can outvote.
    #[test]
    fn an_incomplete_source_blocks_however_clean_the_records_are() {
        let report = score_batch(&clean(1_000_000), &ValidationReport::new(), false);
        assert_eq!(report.grade, QualityGrade::F);
        assert!(report.blocks_release());
        assert_eq!(report.blockers, vec![ReleaseBlocker::IncompleteSources]);
    }

    /// An imbalance cannot be built through the counting API — `observe`
    /// increments `ingested` and exactly one outcome — so the only way one
    /// arrives is from storage. That is itself the property worth having, and
    /// it is why this test needs the `serde` feature to construct its input.
    #[cfg(feature = "serde")]
    #[test]
    fn an_unbalanced_batch_blocks() {
        let broken: BatchReconciliation = serde_json::from_str(
            r#"{"ingested":10,"billable":3,"suppressed":0,"duplicate":0,"rejected":0,"held":0}"#,
        )
        .unwrap();
        let report = score_batch(&broken, &ValidationReport::new(), true);
        assert_eq!(report.grade, QualityGrade::F);
        assert!(report.blockers.contains(&ReleaseBlocker::Unbalanced));
    }

    #[test]
    fn a_blocking_finding_blocks() {
        let findings: ValidationReport =
            vec![Finding::not_keyable(0, NotKeyable::NoCorrelatorPresent)]
                .into_iter()
                .collect();
        let report = score_batch(&clean(10), &findings, true);
        assert_eq!(report.grade, QualityGrade::F);
        assert_eq!(report.blocking_findings, 1);
        assert_eq!(report.warnings, 0);
        assert_eq!(report.blockers, vec![ReleaseBlocker::BlockingFindings]);
    }

    /// Every reason is reported, so fixing one does not imply the batch is
    /// releasable.
    #[cfg(feature = "serde")]
    #[test]
    fn all_blockers_are_reported_not_just_the_first() {
        let broken: BatchReconciliation = serde_json::from_str(
            r#"{"ingested":10,"billable":3,"suppressed":0,"duplicate":0,"rejected":0,"held":0}"#,
        )
        .unwrap();
        let findings: ValidationReport = vec![Finding::sequence_gap("pgw-01", &[7])]
            .into_iter()
            .collect();

        let report = score_batch(&broken, &findings, false);
        assert_eq!(report.blockers, ReleaseBlocker::ALL.to_vec());
        assert!(report.to_string().contains("UNBALANCED"), "{report}");
    }

    /// Grades are proportional, so scale does not change the verdict for the
    /// same defect rate.
    #[test]
    fn grading_is_by_proportion_not_by_count() {
        // One warning in a hundred is one percent — still a working batch.
        assert_eq!(
            score_batch(&clean(100), &warnings(1), true).grade,
            QualityGrade::B
        );
        // Ten thousand warnings in a million is the same rate, same grade.
        assert_eq!(
            score_batch(&clean(1_000_000), &warnings(10_000), true).grade,
            QualityGrade::B
        );
        // Two percent is noisy enough to look at.
        assert_eq!(
            score_batch(&clean(100), &warnings(2), true).grade,
            QualityGrade::C
        );
    }

    /// Held records do not block, but they do stop a period closing.
    #[test]
    fn held_records_grade_down_without_blocking() {
        let mut dispositions = vec![Disposition::Billable; 95];
        dispositions.extend(vec![Disposition::Held(HoldReason::AwaitingPartial); 5]);
        let report = score_batch(&counts(dispositions), &ValidationReport::new(), true);

        assert!(!report.blocks_release(), "held is not a fault");
        assert!(!report.is_final(), "…but the period cannot close");
        assert_eq!(report.grade, QualityGrade::B);
        assert_eq!(report.held, 5);

        // A batch that is half held is deteriorating.
        let mut heavy = vec![Disposition::Billable; 50];
        heavy.extend(vec![Disposition::Held(HoldReason::SequenceGap); 50]);
        assert_eq!(
            score_batch(&counts(heavy), &ValidationReport::new(), true).grade,
            QualityGrade::C
        );
    }

    /// Suppressions and rejections are understood outcomes, not noise, so they
    /// do not by themselves grade a batch down.
    #[test]
    fn understood_outcomes_do_not_degrade_the_grade() {
        let batch = counts(vec![
            Disposition::Billable,
            Disposition::Suppressed(SuppressReason::TestIdentity),
            Disposition::Rejected(RejectReason::Malformed),
            Disposition::Duplicate(RecordKey::RecordingEntity {
                entity: "msc-01".parse().unwrap(),
                local_sequence: 1,
            }),
        ]);
        let report = score_batch(&batch, &ValidationReport::new(), true);
        assert_eq!(report.grade, QualityGrade::A);
        assert!(report.is_final());
    }

    #[test]
    fn an_empty_batch_is_clean_and_says_nothing_else() {
        let report = score_batch(&BatchReconciliation::EMPTY, &ValidationReport::new(), true);
        assert_eq!(report.grade, QualityGrade::A);
        assert!(report.is_final());
    }

    #[test]
    fn grades_round_trip_and_only_f_blocks() {
        for grade in QualityGrade::ALL {
            assert_eq!(grade.as_str().parse::<QualityGrade>().unwrap(), *grade);
            assert_eq!(grade.blocks_release(), *grade == QualityGrade::F);
        }
        assert!("Z".parse::<QualityGrade>().is_err());
        // Grades order from cleanest to worst.
        assert!(QualityGrade::A < QualityGrade::B);
        assert!(QualityGrade::C < QualityGrade::F);
    }
}
