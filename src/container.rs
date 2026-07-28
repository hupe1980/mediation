//! Per-service usage inside one record.
//!
//! A packet-data record does not carry one volume. It carries a **list of
//! containers** (3GPP TS 32.298 `ListOfServiceData`, and the
//! `MultipleUnitUsage` of TS 32.255 in 5G), each scoped to a rating group, a
//! quality class, and the condition that closed it.
//!
//! # Why the list matters
//!
//! One session can cross a tariff boundary, change quality class and change
//! serving cell without ever closing. Each of those opens a new container, and
//! the containers are priced differently. A mediation layer that sums them into
//! a single volume at ingest destroys the only information that makes the
//! session ratable — and, as with uplink and downlink, the loss is not
//! recoverable afterwards.
//!
//! # The record total and the container sum must agree
//!
//! When a record carries both, they are two statements of one fact and can
//! disagree. [`ContainerSet::checked_total`] gives the sum; comparing it with
//! the record's own total is validation rule
//! [`Rule::ContainerSumMismatch`](crate::validation::Rule::ContainerSumMismatch).
//! The crate reports the disagreement rather than picking a side, because
//! either number may be the wrong one and only the operator knows which node
//! is misbehaving.
//!
//! ```rust
//! use mediation::{ChangeCondition, ContainerSet, Octets, RatingGroup, ServiceDataContainer,
//!                 VolumePair};
//!
//! let containers = ContainerSet::from(vec![
//!     ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::TariffTime)
//!         .with_volume(VolumePair::new(Octets::new(1_000), Octets::new(4_000))),
//!     ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::RecordClosure)
//!         .with_volume(VolumePair::new(Octets::new(500), Octets::new(2_000))),
//! ]);
//!
//! assert_eq!(
//!     containers.checked_total(),
//!     Some(VolumePair::new(Octets::new(1_500), Octets::new(6_000))),
//! );
//! // Two containers, one rating group: the split is by tariff period.
//! assert_eq!(containers.rating_groups().len(), 1);
//! ```

use std::fmt;
use std::str::FromStr;

use crate::cdrtime::{CdrTimestamp, Seconds};
use crate::error::ParseError;
use crate::volume::VolumePair;

// ── RatingGroup and Qos5qi ───────────────────────────────────────────────────

/// A rating group — the charging key a container's usage is priced under.
///
/// Opaque to this crate: it identifies a bucket that a rating engine
/// understands, and nothing here interprets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RatingGroup(u32);

impl RatingGroup {
    /// A rating group from its raw value.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// The raw value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for RatingGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for RatingGroup {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.trim()
            .parse::<u32>()
            .map(Self)
            .map_err(|_| ParseError::format("RatingGroup", s, "a 32-bit unsigned decimal integer"))
    }
}

/// A 5G QoS Identifier — the quality class a container's traffic ran under.
///
/// Standardised values occupy 1…127 (3GPP TS 23.501); the rest are operator
/// specific. Both are accepted, and the distinction is exposed rather than
/// enforced, because an operator-specific class is perfectly valid usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Qos5qi(u8);

impl Qos5qi {
    /// A quality class from its raw value. `None` for zero, which is unassigned.
    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    /// The raw value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// `true` for the standardised range 1…127.
    #[must_use]
    pub const fn is_standardised(self) -> bool {
        self.0 <= 127
    }
}

impl fmt::Display for Qos5qi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Qos5qi {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("Qos5qi", s, "a 5QI value in 1..=255");
        Self::new(s.trim().parse::<u8>().map_err(|_| err())?).ok_or_else(err)
    }
}

// ── ChangeCondition ──────────────────────────────────────────────────────────

/// Why a container was closed (3GPP TS 32.298 `ChangeCondition`).
///
/// The condition determines whether the split was **priced** — a tariff-time
/// boundary opens a container precisely so the two sides can be rated
/// differently — or merely **mechanical**, as when a reporting limit was
/// reached. Losing it makes the containers uninterpretable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChangeCondition {
    /// The quality class changed.
    QosChange,
    /// A tariff period boundary was crossed.
    TariffTime,
    /// The record itself closed.
    RecordClosure,
    /// The reported volume limit was reached.
    VolumeLimit,
    /// The reported time limit was reached.
    TimeLimit,
    /// The serving cell changed.
    CellChange,
    /// The routing area changed.
    RoutingAreaChange,
    /// A direct-tunnel was established.
    TunnelEstablishment,
    /// A direct-tunnel was removed.
    TunnelRemoval,
    /// The serving network changed.
    ServingNodeChange,
    /// The radio access technology changed.
    RatChange,
}

impl ChangeCondition {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[
        Self::QosChange,
        Self::TariffTime,
        Self::RecordClosure,
        Self::VolumeLimit,
        Self::TimeLimit,
        Self::CellChange,
        Self::RoutingAreaChange,
        Self::TunnelEstablishment,
        Self::TunnelRemoval,
        Self::ServingNodeChange,
        Self::RatChange,
    ];

    /// The canonical codes, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] = &[
        "QOS_CHANGE",
        "TARIFF_TIME",
        "RECORD_CLOSURE",
        "VOLUME_LIMIT",
        "TIME_LIMIT",
        "CELL_CHANGE",
        "ROUTING_AREA_CHANGE",
        "TUNNEL_ESTABLISHMENT",
        "TUNNEL_REMOVAL",
        "SERVING_NODE_CHANGE",
        "RAT_CHANGE",
    ];

    /// `true` when the split exists so the two sides can be **priced**
    /// differently.
    ///
    /// Merging containers across such a boundary rates the whole session at one
    /// side's tariff, which is a billing error rather than a rounding one.
    #[must_use]
    pub const fn is_priced_boundary(self) -> bool {
        matches!(
            self,
            Self::TariffTime | Self::QosChange | Self::RatChange | Self::ServingNodeChange
        )
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::QosChange => "QOS_CHANGE",
            Self::TariffTime => "TARIFF_TIME",
            Self::RecordClosure => "RECORD_CLOSURE",
            Self::VolumeLimit => "VOLUME_LIMIT",
            Self::TimeLimit => "TIME_LIMIT",
            Self::CellChange => "CELL_CHANGE",
            Self::RoutingAreaChange => "ROUTING_AREA_CHANGE",
            Self::TunnelEstablishment => "TUNNEL_ESTABLISHMENT",
            Self::TunnelRemoval => "TUNNEL_REMOVAL",
            Self::ServingNodeChange => "SERVING_NODE_CHANGE",
            Self::RatChange => "RAT_CHANGE",
        }
    }
}

impl fmt::Display for ChangeCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ChangeCondition {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_uppercase();
        Self::ALL
            .iter()
            .find(|c| c.as_str() == t)
            .copied()
            .ok_or_else(|| ParseError::one_of("ChangeCondition", s, Self::CODES))
    }
}

// ── ServiceDataContainer ─────────────────────────────────────────────────────

/// One rating group's usage within a record, up to the point it was closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDataContainer {
    /// The charging bucket this usage is priced under.
    pub rating_group: RatingGroup,
    /// Why the container was closed.
    pub change_condition: ChangeCondition,
    /// The quality class the traffic ran under, where the record said.
    pub qos: Option<Qos5qi>,
    /// Transferred volume, uplink and downlink kept apart.
    pub volume: VolumePair,
    /// Chargeable time within this container, for time-based rating.
    pub duration: Option<Seconds>,
    /// When the container was closed, where the record said.
    pub closed: Option<CdrTimestamp>,
}

impl ServiceDataContainer {
    /// A container with no usage recorded yet.
    #[must_use]
    pub const fn new(rating_group: RatingGroup, change_condition: ChangeCondition) -> Self {
        Self {
            rating_group,
            change_condition,
            qos: None,
            volume: VolumePair::ZERO,
            duration: None,
            closed: None,
        }
    }

    /// The same container with a transferred volume.
    #[must_use]
    pub const fn with_volume(mut self, volume: VolumePair) -> Self {
        self.volume = volume;
        self
    }

    /// The same container with a chargeable duration.
    #[must_use]
    pub const fn with_duration(mut self, duration: Seconds) -> Self {
        self.duration = Some(duration);
        self
    }

    /// The same container with a quality class.
    #[must_use]
    pub const fn with_qos(mut self, qos: Qos5qi) -> Self {
        self.qos = Some(qos);
        self
    }

    /// The same container with a close time.
    #[must_use]
    pub const fn with_closed(mut self, closed: CdrTimestamp) -> Self {
        self.closed = Some(closed);
        self
    }

    /// `true` when the container recorded no usage at all.
    #[must_use]
    pub fn is_zero_usage(&self) -> bool {
        self.volume.is_zero() && self.duration.is_none_or(Seconds::is_zero)
    }
}

// ── ContainerSet ─────────────────────────────────────────────────────────────

/// The containers of one record, in the order the record carried them.
///
/// A newtype rather than a bare `Vec` so the arithmetic that must be checked —
/// summing volumes across containers — has one home and cannot be open-coded
/// with a wrapping `+`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerSet(Vec<ServiceDataContainer>);

impl ContainerSet {
    /// No containers.
    #[must_use]
    pub const fn empty() -> Self {
        Self(Vec::new())
    }

    /// The containers, in record order.
    #[must_use]
    pub fn as_slice(&self) -> &[ServiceDataContainer] {
        &self.0
    }

    /// How many containers the record carried.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` when the record carried no containers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The volume across all containers, `None` on overflow.
    ///
    /// Checked, and uplink and downlink stay apart throughout.
    #[must_use]
    pub fn checked_total(&self) -> Option<VolumePair> {
        self.0
            .iter()
            .try_fold(VolumePair::ZERO, |acc, c| acc.checked_add(c.volume))
    }

    /// The chargeable time across all containers.
    ///
    /// `None` unless **every** container carried a duration, and `None` on
    /// overflow. A partial sum is not a total: treating the containers that
    /// carried no duration as zero would understate the session by exactly the
    /// time nobody reported, in the direction that costs revenue.
    ///
    /// Many containers legitimately carry no duration — volume-rated usage does
    /// not need one — so an absent total here is normal rather than a fault.
    #[must_use]
    pub fn checked_total_duration(&self) -> Option<Seconds> {
        self.0
            .iter()
            .try_fold(Seconds::ZERO, |acc, c| acc.checked_add(c.duration?))
    }

    /// The distinct rating groups present, ascending.
    ///
    /// Several containers commonly share one rating group — a tariff boundary
    /// splits the usage without changing what it is priced as.
    #[must_use]
    pub fn rating_groups(&self) -> Vec<RatingGroup> {
        let mut groups: Vec<RatingGroup> = self.0.iter().map(|c| c.rating_group).collect();
        groups.sort_unstable();
        groups.dedup();
        groups
    }

    /// The volume for one rating group, `None` on overflow.
    #[must_use]
    pub fn total_for(&self, group: RatingGroup) -> Option<VolumePair> {
        self.0
            .iter()
            .filter(|c| c.rating_group == group)
            .try_fold(VolumePair::ZERO, |acc, c| acc.checked_add(c.volume))
    }

    /// `true` when any container was closed at a boundary that affects pricing.
    ///
    /// The signal that the containers must **not** be flattened before rating.
    #[must_use]
    pub fn crosses_priced_boundary(&self) -> bool {
        self.0
            .iter()
            .any(|c| c.change_condition.is_priced_boundary())
    }

    /// Whether the containers agree with a record-level total.
    ///
    /// `None` when the record carries no total to compare against, or when the
    /// container sum overflows. Otherwise `Some(true)` when they agree.
    #[must_use]
    pub fn agrees_with(&self, record_total: Option<VolumePair>) -> Option<bool> {
        let declared = record_total?;
        let summed = self.checked_total()?;
        Some(declared == summed)
    }

    /// The difference between the record's total and the container sum, in
    /// octets, for each direction.
    ///
    /// `None` when either is unavailable. Present so a report can say *how far*
    /// out a mismatch is, which is what distinguishes a rounding artefact from
    /// a dropped container.
    #[must_use]
    pub fn discrepancy(&self, record_total: Option<VolumePair>) -> Option<(i128, i128)> {
        let declared = record_total?;
        let summed = self.checked_total()?;
        Some((
            i128::from(declared.uplink().get()) - i128::from(summed.uplink().get()),
            i128::from(declared.downlink().get()) - i128::from(summed.downlink().get()),
        ))
    }
}

impl From<Vec<ServiceDataContainer>> for ContainerSet {
    fn from(containers: Vec<ServiceDataContainer>) -> Self {
        Self(containers)
    }
}

impl FromIterator<ServiceDataContainer> for ContainerSet {
    fn from_iter<I: IntoIterator<Item = ServiceDataContainer>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<'a> IntoIterator for &'a ContainerSet {
    type Item = &'a ServiceDataContainer;
    type IntoIter = std::slice::Iter<'a, ServiceDataContainer>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl fmt::Display for ContainerSet {
    /// Writes the container count and the summed volume, compact enough for a
    /// report line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.checked_total() {
            Some(total) => write!(f, "{} container(s), {total}", self.0.len()),
            None => write!(f, "{} container(s), total overflows", self.0.len()),
        }
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{ChangeCondition, ContainerSet, Qos5qi, RatingGroup, ServiceDataContainer};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for ChangeCondition {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for ChangeCondition {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = ChangeCondition;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a change condition code such as \"TARIFF_TIME\"")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<ChangeCondition, E> {
                    v.parse().map_err(de::Error::custom)
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    macro_rules! number_serde {
        ($ty:ty, $inner:ty, $make:expr) => {
            impl Serialize for $ty {
                fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                    self.get().serialize(serializer)
                }
            }

            impl<'de> Deserialize<'de> for $ty {
                fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                    let raw = <$inner>::deserialize(deserializer)?;
                    #[allow(clippy::redundant_closure_call)]
                    ($make)(raw)
                }
            }
        };
    }

    number_serde!(RatingGroup, u32, |raw| Ok(RatingGroup::new(raw)));
    number_serde!(Qos5qi, u8, |raw| Qos5qi::new(raw)
        .ok_or_else(|| de::Error::custom("5QI 0 is unassigned")));

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "ServiceDataContainer")]
    struct ContainerRepr {
        rating_group: RatingGroup,
        change_condition: ChangeCondition,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        qos_5qi: Option<Qos5qi>,
        volume: crate::volume::VolumePair,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_s: Option<crate::cdrtime::Seconds>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        closed: Option<crate::cdrtime::CdrTimestamp>,
    }

    impl Serialize for ServiceDataContainer {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            ContainerRepr {
                rating_group: self.rating_group,
                change_condition: self.change_condition,
                qos_5qi: self.qos,
                volume: self.volume,
                duration_s: self.duration,
                closed: self.closed,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for ServiceDataContainer {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = ContainerRepr::deserialize(deserializer)?;
            Ok(ServiceDataContainer {
                rating_group: r.rating_group,
                change_condition: r.change_condition,
                qos: r.qos_5qi,
                volume: r.volume,
                duration: r.duration_s,
                closed: r.closed,
            })
        }
    }

    impl Serialize for ContainerSet {
        /// A plain array: the containers are ordered and the order is data.
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.as_slice().serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for ContainerSet {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Vec::<ServiceDataContainer>::deserialize(deserializer).map(ContainerSet::from)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volume::Octets;

    fn container(
        group: u32,
        condition: ChangeCondition,
        up: u64,
        down: u64,
    ) -> ServiceDataContainer {
        ServiceDataContainer::new(RatingGroup::new(group), condition)
            .with_volume(VolumePair::new(Octets::new(up), Octets::new(down)))
    }

    #[test]
    fn containers_sum_with_the_directions_kept_apart() {
        let set = ContainerSet::from(vec![
            container(10, ChangeCondition::TariffTime, 1_000, 4_000),
            container(10, ChangeCondition::RecordClosure, 500, 2_000),
        ]);
        assert_eq!(
            set.checked_total(),
            Some(VolumePair::new(Octets::new(1_500), Octets::new(6_000)))
        );
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn a_summed_overflow_is_reported_rather_than_wrapped() {
        let set = ContainerSet::from(vec![
            container(1, ChangeCondition::RecordClosure, u64::MAX, 0),
            container(1, ChangeCondition::RecordClosure, 1, 0),
        ]);
        assert_eq!(set.checked_total(), None);
        assert_eq!(set.agrees_with(Some(VolumePair::ZERO)), None);
        assert!(set.to_string().contains("overflows"), "{set}");
    }

    /// Several containers commonly share a rating group: the split is by tariff
    /// period, not by what the usage is priced as.
    #[test]
    fn one_rating_group_may_span_several_containers() {
        let set = ContainerSet::from(vec![
            container(10, ChangeCondition::TariffTime, 1_000, 0),
            container(10, ChangeCondition::RecordClosure, 500, 0),
            container(20, ChangeCondition::RecordClosure, 250, 0),
        ]);
        assert_eq!(
            set.rating_groups(),
            vec![RatingGroup::new(10), RatingGroup::new(20)]
        );
        assert_eq!(
            set.total_for(RatingGroup::new(10)),
            Some(VolumePair::new(Octets::new(1_500), Octets::ZERO))
        );
        assert_eq!(
            set.total_for(RatingGroup::new(99)),
            Some(VolumePair::ZERO),
            "an absent group has no usage, which is a zero rather than an error"
        );
    }

    /// The reason the list must survive ingest: flattening across a priced
    /// boundary rates the whole session at one side's tariff.
    #[test]
    fn a_priced_boundary_is_distinguishable_from_a_mechanical_split() {
        let priced = ContainerSet::from(vec![
            container(10, ChangeCondition::TariffTime, 1, 1),
            container(10, ChangeCondition::RecordClosure, 1, 1),
        ]);
        assert!(priced.crosses_priced_boundary());

        let mechanical = ContainerSet::from(vec![
            container(10, ChangeCondition::VolumeLimit, 1, 1),
            container(10, ChangeCondition::RecordClosure, 1, 1),
        ]);
        assert!(!mechanical.crosses_priced_boundary());

        for condition in ChangeCondition::ALL {
            let expected = matches!(
                condition,
                ChangeCondition::TariffTime
                    | ChangeCondition::QosChange
                    | ChangeCondition::RatChange
                    | ChangeCondition::ServingNodeChange
            );
            assert_eq!(condition.is_priced_boundary(), expected, "{condition}");
        }
    }

    /// Two statements of one fact can disagree, and the crate reports the
    /// disagreement rather than picking a side.
    #[test]
    fn a_record_total_is_checked_against_the_container_sum() {
        let set = ContainerSet::from(vec![container(
            10,
            ChangeCondition::RecordClosure,
            100,
            900,
        )]);

        let agreeing = VolumePair::new(Octets::new(100), Octets::new(900));
        assert_eq!(set.agrees_with(Some(agreeing)), Some(true));
        assert_eq!(set.discrepancy(Some(agreeing)), Some((0, 0)));

        // A dropped container shows up as a positive discrepancy …
        let declared = VolumePair::new(Octets::new(150), Octets::new(900));
        assert_eq!(set.agrees_with(Some(declared)), Some(false));
        assert_eq!(set.discrepancy(Some(declared)), Some((50, 0)));

        // … and an over-count as a negative one, so the sign says which side is
        // short.
        let under = VolumePair::new(Octets::new(80), Octets::new(900));
        assert_eq!(set.discrepancy(Some(under)), Some((-20, 0)));

        // Nothing to compare against is not a disagreement.
        assert_eq!(set.agrees_with(None), None);
    }

    /// A partial sum is not a total: one container without a duration makes the
    /// session's chargeable time unknowable, not shorter.
    #[test]
    fn a_duration_total_needs_every_container_to_carry_one() {
        let volumes_only =
            ContainerSet::from(vec![container(10, ChangeCondition::RecordClosure, 1, 1)]);
        assert_eq!(volumes_only.checked_total_duration(), None);

        let mixed = ContainerSet::from(vec![
            container(10, ChangeCondition::TariffTime, 1, 1).with_duration(Seconds::new(30)),
            container(10, ChangeCondition::RecordClosure, 1, 1),
        ]);
        assert_eq!(
            mixed.checked_total_duration(),
            None,
            "30 s would understate the session by whatever the second container ran for"
        );

        let complete = ContainerSet::from(vec![
            container(10, ChangeCondition::TariffTime, 1, 1).with_duration(Seconds::new(30)),
            container(10, ChangeCondition::RecordClosure, 1, 1).with_duration(Seconds::new(12)),
        ]);
        assert_eq!(complete.checked_total_duration(), Some(Seconds::new(42)));
    }

    #[test]
    fn an_empty_set_totals_to_zero_and_has_no_groups() {
        let set = ContainerSet::empty();
        assert!(set.is_empty());
        assert_eq!(set.checked_total(), Some(VolumePair::ZERO));
        assert!(set.rating_groups().is_empty());
        assert!(!set.crosses_priced_boundary());
    }

    #[test]
    fn quality_classes_reject_the_unassigned_zero() {
        assert_eq!(Qos5qi::new(0), None);
        assert!("0".parse::<Qos5qi>().is_err());
        assert!(Qos5qi::new(9).unwrap().is_standardised());
        assert!(!Qos5qi::new(200).unwrap().is_standardised());
        assert_eq!("9".parse::<Qos5qi>().unwrap().to_string(), "9");
    }

    #[test]
    fn change_condition_codes_round_trip() {
        assert_eq!(ChangeCondition::ALL.len(), ChangeCondition::CODES.len());
        for (condition, code) in ChangeCondition::ALL.iter().zip(ChangeCondition::CODES) {
            assert_eq!(condition.as_str(), *code);
            assert_eq!(code.parse::<ChangeCondition>().unwrap(), *condition);
        }
        assert!("NOPE".parse::<ChangeCondition>().is_err());
    }
}
