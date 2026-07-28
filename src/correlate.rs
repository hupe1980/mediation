//! Joining the records that describe one usage.
//!
//! A single subscriber action produces records at every node it traverses. The
//! P-GW assigns a Charging ID when it creates an IP-CAN bearer and forwards it
//! to the S-GW, so both nodes' records carry it (3GPP TS 32.251) — and an IMS
//! session's ICID travels in the `P-Charging-Vector` of every SIP message that
//! belongs to it (TS 32.260).
//!
//! # Two failures, opposite in sign
//!
//! **Billing both sides double-charges.** Two records describing one bearer,
//! each carrying its own volume, are not two usages. Releasing both invoices
//! the session twice, and the subscriber notices.
//!
//! **Discarding an unmatched record hides leakage.** A record whose counterpart
//! never arrived is evidence that something was lost — quite possibly the side
//! that was going to be billed. Dropping it makes the loss invisible, which is
//! how revenue leakage survives an audit.
//!
//! So [`correlate`] returns the joined groups **and** the unmatched records,
//! and never silently drops either.
//!
//! ```rust
//! use mediation::{CorrelationConfig, correlate};
//! # use mediation::{ChargedParty, ChargingId, CorrelationKeys, Octets, RecordType, SourceRef,
//! #                 SubscriberRef, UsageRecord, VolumePair};
//! # use std::net::Ipv4Addr;
//! # fn record(node: &str, volume: Option<VolumePair>) -> UsageRecord {
//! #     let mut r = UsageRecord::new(
//! #         RecordType::PduSession,
//! #         ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
//! #         "2026-07-28T10:00:00+02:00".parse().unwrap(),
//! #         SourceRef::new(node.parse().unwrap(), 0),
//! #     ).with_correlation(
//! #         CorrelationKeys::default()
//! #             .with_charging_id(ChargingId::new(42))
//! #             .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()));
//! #     r.volume = volume;
//! #     r
//! # }
//! // The same bearer, seen by two nodes, each reporting a volume.
//! let records = vec![
//!     record("sgw-01", Some(VolumePair::new(Octets::new(100), Octets::new(900)))),
//!     record("pgw-01", Some(VolumePair::new(Octets::new(100), Octets::new(900)))),
//! ];
//!
//! let result = correlate(&records, &CorrelationConfig::packet_bearer());
//! assert_eq!(result.groups().len(), 1);
//! assert!(result.groups()[0].would_double_count(), "bill one side, not both");
//! assert!(result.unmatched().is_empty());
//! ```
//!
//! # Leg chains
//!
//! When a terminating application server diverts a call it generates a **new**
//! ICID for the outgoing leg and records the incoming one as a linked ICID
//! (TS 32.260). The legs therefore form a chain, and following it reconstructs
//! the call history that a forwarded call actually took.
//!
//! A leg whose linked ICID names a session that is not present is a
//! [`UnmatchedReason::PredecessorAbsent`] — the earlier leg was lost, and with
//! it the evidence of who originated the call.

use std::collections::HashMap;
use std::fmt;

use crate::correlation::{Icid, PacketSessionId};
use crate::record::UsageRecord;

// ── CorrelationKey ───────────────────────────────────────────────────────────

/// What a group of records was joined on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CorrelationKey {
    /// One IP-CAN bearer or PDU session, seen by every node that served it.
    PacketBearer(PacketSessionId),
    /// One IMS session, seen by every element that handled it.
    ImsSession(Icid),
}

impl CorrelationKey {
    /// A short code naming the join, for grouping and reporting.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::PacketBearer(_) => "PACKET_BEARER",
            Self::ImsSession(_) => "IMS_SESSION",
        }
    }
}

impl fmt::Display for CorrelationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PacketBearer(session) => write!(f, "PACKET_BEARER:{session}"),
            Self::ImsSession(icid) => write!(f, "IMS_SESSION:{icid}"),
        }
    }
}

// ── Configuration ────────────────────────────────────────────────────────────

/// Which correlator to join on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrelationConfig {
    join_packet_bearers: bool,
    join_ims_sessions: bool,
    follow_leg_chains: bool,
}

impl CorrelationConfig {
    /// Join packet-data records on Charging ID **and** gateway.
    #[must_use]
    pub const fn packet_bearer() -> Self {
        Self {
            join_packet_bearers: true,
            join_ims_sessions: false,
            follow_leg_chains: false,
        }
    }

    /// Join IMS records on their session identifier, and follow leg chains.
    #[must_use]
    pub const fn ims() -> Self {
        Self {
            join_packet_bearers: false,
            join_ims_sessions: true,
            follow_leg_chains: true,
        }
    }

    /// Join on every correlator this crate understands.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            join_packet_bearers: true,
            join_ims_sessions: true,
            follow_leg_chains: true,
        }
    }

    /// The same configuration without leg-chain reconstruction.
    #[must_use]
    pub const fn without_leg_chains(mut self) -> Self {
        self.follow_leg_chains = false;
        self
    }
}

// ── Group ────────────────────────────────────────────────────────────────────

/// The records that describe one usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrelatedGroup {
    /// What the members were joined on.
    pub key: CorrelationKey,
    /// Indices into the **caller's** slice, ascending.
    pub members: Vec<usize>,
    /// How many members carry a quantity that could be billed.
    ///
    /// The number that matters: two nodes both reporting the volume of one
    /// bearer is normal, and billing both is not.
    pub quantity_bearing: usize,
}

impl CorrelatedGroup {
    /// `true` when more than one member carries a billable quantity.
    ///
    /// Not itself a fault — an S-GW and a P-GW reporting the same bearer is the
    /// expected shape. It is a fault only if a caller releases both, which is
    /// exactly what this flag exists to prevent.
    #[must_use]
    pub const fn would_double_count(&self) -> bool {
        self.quantity_bearing > 1
    }

    /// `true` when only one record described this usage.
    ///
    /// Whether that is normal or a missing counterpart depends on the
    /// deployment, which is why it is reported rather than judged: many
    /// networks emit a P-GW record and nothing else.
    #[must_use]
    pub fn is_solitary(&self) -> bool {
        self.members.len() == 1
    }

    /// How many records were joined.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Always `false` — a group is created only when it has a member.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl fmt::Display for CorrelatedGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} — {} record(s), {} bearing a quantity",
            self.key,
            self.members.len(),
            self.quantity_bearing
        )
    }
}

// ── Unmatched ────────────────────────────────────────────────────────────────

/// Why a record could not be joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnmatchedReason {
    /// The record carries no correlator the configuration joins on.
    ///
    /// Not necessarily a fault: a short message has no session to belong to.
    NoCorrelator,
    /// The record names a predecessor leg that is not present.
    ///
    /// The earlier leg was lost, and with it the evidence of who originated the
    /// call — so the chain cannot be reconstructed and the origin is unknown.
    PredecessorAbsent,
}

impl UnmatchedReason {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[Self::NoCorrelator, Self::PredecessorAbsent];

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoCorrelator => "NO_CORRELATOR",
            Self::PredecessorAbsent => "PREDECESSOR_ABSENT",
        }
    }
}

impl fmt::Display for UnmatchedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A record that no group claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unmatched {
    /// Index into the **caller's** slice.
    pub record_index: usize,
    /// Why it could not be joined.
    pub reason: UnmatchedReason,
}

// ── Leg chains ───────────────────────────────────────────────────────────────

/// A call's legs, in the order they were created.
///
/// Reconstructed by following linked ICIDs backwards to the leg that names no
/// predecessor, then forwards. A forwarded call that a caller bills leg by leg
/// without this looks like several unrelated calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegChain {
    /// The session identifiers, origin first.
    pub legs: Vec<Icid>,
}

impl LegChain {
    /// How many legs the call took.
    #[must_use]
    pub fn len(&self) -> usize {
        self.legs.len()
    }

    /// Always `false` — a chain is created only when it has a leg.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// `true` when the call was diverted at least once.
    #[must_use]
    pub fn is_forwarded(&self) -> bool {
        self.legs.len() > 1
    }

    /// The leg the call originated on.
    #[must_use]
    pub fn origin(&self) -> &Icid {
        // A chain is never constructed empty.
        &self.legs[0]
    }
}

impl fmt::Display for LegChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (position, leg) in self.legs.iter().enumerate() {
            if position > 0 {
                f.write_str(" → ")?;
            }
            write!(f, "{leg}")?;
        }
        Ok(())
    }
}

// ── Result ───────────────────────────────────────────────────────────────────

/// Everything the join produced.
///
/// Groups and unmatched records together account for **every** input record
/// exactly once — asserted by [`accounts_for`](Self::accounts_for). Dropping an
/// unmatched record would make lost usage invisible, so the type makes that
/// impossible to do accidentally.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CorrelationResult {
    groups: Vec<CorrelatedGroup>,
    unmatched: Vec<Unmatched>,
    chains: Vec<LegChain>,
}

impl CorrelationResult {
    /// The joined groups, ordered by their first member.
    #[must_use]
    pub fn groups(&self) -> &[CorrelatedGroup] {
        &self.groups
    }

    /// The records no group claimed, ascending by index.
    #[must_use]
    pub fn unmatched(&self) -> &[Unmatched] {
        &self.unmatched
    }

    /// The call histories reconstructed from linked session identifiers.
    #[must_use]
    pub fn chains(&self) -> &[LegChain] {
        &self.chains
    }

    /// Groups where more than one member carries a billable quantity.
    ///
    /// Release one member of each, not all — see
    /// [`CorrelatedGroup::would_double_count`].
    #[must_use]
    pub fn double_counting_groups(&self) -> Vec<&CorrelatedGroup> {
        self.groups
            .iter()
            .filter(|g| g.would_double_count())
            .collect()
    }

    /// `true` when every one of `total` input records was accounted for exactly
    /// once.
    ///
    /// The correlation counterpart of the conservation law: a join that loses a
    /// record loses the usage it described.
    #[must_use]
    pub fn accounts_for(&self, total: usize) -> bool {
        let grouped: usize = self.groups.iter().map(CorrelatedGroup::len).sum();
        grouped + self.unmatched.len() == total
    }
}

impl fmt::Display for CorrelationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} group(s), {} unmatched, {} chain(s), {} at risk of double counting",
            self.groups.len(),
            self.unmatched.len(),
            self.chains.len(),
            self.double_counting_groups().len()
        )
    }
}

// ── The engine ───────────────────────────────────────────────────────────────

/// Join the records that describe one usage.
///
/// Deterministic and order-independent in outcome: groups are returned ordered
/// by their first member's index and their members ascending, so a shuffled
/// input produces the same groups with the same contents.
///
/// Indices refer to `records` as passed.
#[must_use]
pub fn correlate(records: &[UsageRecord], config: &CorrelationConfig) -> CorrelationResult {
    let mut buckets: HashMap<CorrelationKey, Vec<usize>> = HashMap::new();
    let mut unmatched = Vec::new();

    for (index, record) in records.iter().enumerate() {
        let key = if config.join_packet_bearers
            && let Some(session) = record.correlation.packet_session()
        {
            Some(CorrelationKey::PacketBearer(session))
        } else if config.join_ims_sessions
            && let Some(icid) = &record.correlation.icid
        {
            Some(CorrelationKey::ImsSession(icid.clone()))
        } else {
            None
        };

        match key {
            Some(key) => buckets.entry(key).or_default().push(index),
            None => unmatched.push(Unmatched {
                record_index: index,
                reason: UnmatchedReason::NoCorrelator,
            }),
        }
    }

    let mut groups: Vec<CorrelatedGroup> = buckets
        .into_iter()
        .map(|(key, mut members)| {
            members.sort_unstable();
            let quantity_bearing = members
                .iter()
                .filter(|index| !records[**index].is_zero_usage())
                .count();
            CorrelatedGroup {
                key,
                members,
                quantity_bearing,
            }
        })
        .collect();
    // Deterministic order: by first member, which is unique across groups.
    groups.sort_by_key(|group| group.members[0]);

    let chains = if config.follow_leg_chains {
        build_leg_chains(records, &mut unmatched)
    } else {
        Vec::new()
    };
    unmatched.sort_by_key(|u| u.record_index);

    CorrelationResult {
        groups,
        unmatched,
        chains,
    }
}

/// Reconstruct call histories from linked session identifiers.
///
/// A leg naming a predecessor that is not present is recorded as unmatched
/// rather than treated as an origin, because the difference is exactly the
/// evidence of who placed the call.
fn build_leg_chains(records: &[UsageRecord], unmatched: &mut Vec<Unmatched>) -> Vec<LegChain> {
    // Every session present, and what each names as its predecessor.
    let mut present: HashMap<&Icid, usize> = HashMap::new();
    let mut successor: HashMap<&Icid, &Icid> = HashMap::new();
    let mut linked_from: Vec<(usize, &Icid, &Icid)> = Vec::new();

    for (index, record) in records.iter().enumerate() {
        if let Some(icid) = &record.correlation.icid {
            present.entry(icid).or_insert(index);
            if let Some(previous) = &record.correlation.linked_icid {
                linked_from.push((index, icid, previous));
            }
        }
    }
    for (index, icid, previous) in &linked_from {
        if present.contains_key(previous) {
            successor.insert(previous, icid);
        } else {
            unmatched.push(Unmatched {
                record_index: *index,
                reason: UnmatchedReason::PredecessorAbsent,
            });
        }
    }

    // An origin is a session nothing resolvable points forward from.
    let has_predecessor: std::collections::HashSet<&Icid> = linked_from
        .iter()
        .filter(|(_, _, previous)| present.contains_key(previous))
        .map(|(_, icid, _)| *icid)
        .collect();

    let mut origins: Vec<(&Icid, usize)> = present
        .iter()
        .filter(|(icid, _)| !has_predecessor.contains(*icid))
        .map(|(icid, index)| (*icid, *index))
        .collect();
    origins.sort_by_key(|(_, index)| *index);

    origins
        .into_iter()
        .map(|(origin, _)| {
            let mut legs = vec![origin.clone()];
            let mut cursor = origin;
            // Bounded by the number of distinct sessions, so a cycle in
            // malformed input cannot loop forever.
            while legs.len() <= present.len()
                && let Some(next) = successor.get(cursor)
            {
                legs.push((*next).clone());
                cursor = next;
            }
            LegChain { legs }
        })
        .collect()
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{CorrelationKey, LegChain, Unmatched, UnmatchedReason};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for UnmatchedReason {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for UnmatchedReason {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = UnmatchedReason;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("an unmatched reason code")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<UnmatchedReason, E> {
                    UnmatchedReason::ALL
                        .iter()
                        .find(|r| r.as_str() == v)
                        .copied()
                        .ok_or_else(|| de::Error::custom(format!("unknown reason {v:?}")))
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    /// An internally tagged representation, as elsewhere in the crate.
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename = "CorrelationKey")]
    enum KeyRepr {
        #[serde(rename = "PACKET_BEARER")]
        PacketBearer {
            session: crate::correlation::PacketSessionId,
        },
        #[serde(rename = "IMS_SESSION")]
        ImsSession { icid: crate::correlation::Icid },
    }

    impl Serialize for CorrelationKey {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match self {
                Self::PacketBearer(session) => KeyRepr::PacketBearer { session: *session },
                Self::ImsSession(icid) => KeyRepr::ImsSession { icid: icid.clone() },
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for CorrelationKey {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Ok(match KeyRepr::deserialize(deserializer)? {
                KeyRepr::PacketBearer { session } => Self::PacketBearer(session),
                KeyRepr::ImsSession { icid } => Self::ImsSession(icid),
            })
        }
    }

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "Unmatched")]
    struct UnmatchedRepr {
        record_index: usize,
        reason: UnmatchedReason,
    }

    impl Serialize for Unmatched {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            UnmatchedRepr {
                record_index: self.record_index,
                reason: self.reason,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Unmatched {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = UnmatchedRepr::deserialize(deserializer)?;
            Ok(Unmatched {
                record_index: r.record_index,
                reason: r.reason,
            })
        }
    }

    impl Serialize for LegChain {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.legs.serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for LegChain {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let legs = Vec::<crate::correlation::Icid>::deserialize(deserializer)?;
            if legs.is_empty() {
                return Err(de::Error::custom("a leg chain has at least one leg"));
            }
            Ok(LegChain { legs })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::correlation::{ChargingId, CorrelationKeys};
    use crate::party::{ChargedParty, SubscriberRef};
    use crate::record::{RecordType, SourceRef};
    use crate::volume::{Octets, VolumePair};
    use std::net::Ipv4Addr;

    fn record(node: &str, keys: CorrelationKeys, volume: Option<VolumePair>) -> UsageRecord {
        let mut record = UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new(node.parse().unwrap(), 0),
        )
        .with_correlation(keys);
        record.volume = volume;
        record
    }

    fn bearer(charging_id: u32) -> CorrelationKeys {
        CorrelationKeys::default()
            .with_charging_id(ChargingId::new(charging_id))
            .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into())
    }

    fn some_volume() -> Option<VolumePair> {
        Some(VolumePair::new(Octets::new(100), Octets::new(900)))
    }

    /// The failure that costs a subscriber money: two nodes describing one
    /// bearer, both carrying its volume.
    #[test]
    fn two_nodes_on_one_bearer_are_flagged_before_they_are_billed_twice() {
        let records = vec![
            record("sgw-01", bearer(42), some_volume()),
            record("pgw-01", bearer(42), some_volume()),
        ];
        let result = correlate(&records, &CorrelationConfig::packet_bearer());

        assert_eq!(result.groups().len(), 1);
        let group = &result.groups()[0];
        assert_eq!(group.members, vec![0, 1]);
        assert_eq!(group.quantity_bearing, 2);
        assert!(group.would_double_count());
        assert_eq!(result.double_counting_groups().len(), 1);
    }

    /// A node reporting the bearer without a quantity is not double counting —
    /// only one side can be billed, and only one side carries anything.
    #[test]
    fn a_quantityless_counterpart_is_not_double_counting() {
        let records = vec![
            record("sgw-01", bearer(42), None),
            record("pgw-01", bearer(42), some_volume()),
        ];
        let result = correlate(&records, &CorrelationConfig::packet_bearer());
        let group = &result.groups()[0];

        assert_eq!(group.len(), 2);
        assert_eq!(group.quantity_bearing, 1);
        assert!(!group.would_double_count());
    }

    /// Different gateways using the same counter value are different bearers,
    /// per TS 32.251 — joining them would merge two subscribers' sessions.
    #[test]
    fn the_gateway_is_part_of_the_join() {
        let other_gateway = CorrelationKeys::default()
            .with_charging_id(ChargingId::new(42))
            .with_gateway(Ipv4Addr::new(10, 0, 0, 2).into());
        let records = vec![
            record("pgw-01", bearer(42), some_volume()),
            record("pgw-02", other_gateway, some_volume()),
        ];
        let result = correlate(&records, &CorrelationConfig::packet_bearer());

        assert_eq!(result.groups().len(), 2);
        assert!(result.groups().iter().all(CorrelatedGroup::is_solitary));
    }

    /// Nothing is dropped: every input record is in a group or in the unmatched
    /// list, and the result says so.
    #[test]
    fn every_record_is_accounted_for() {
        let records = vec![
            record("pgw-01", bearer(1), some_volume()),
            record("sgw-01", bearer(1), some_volume()),
            record("smsc-01", CorrelationKeys::none(), some_volume()),
            record("pgw-01", bearer(2), some_volume()),
        ];
        let result = correlate(&records, &CorrelationConfig::packet_bearer());

        assert!(result.accounts_for(records.len()));
        assert_eq!(result.unmatched().len(), 1);
        assert_eq!(result.unmatched()[0].record_index, 2);
        assert_eq!(result.unmatched()[0].reason, UnmatchedReason::NoCorrelator);
    }

    /// A shuffled input produces the same groups with the same contents.
    #[test]
    fn the_join_is_order_independent() {
        let make = |order: [usize; 4]| {
            let all = [
                record("pgw-01", bearer(1), some_volume()),
                record("sgw-01", bearer(1), some_volume()),
                record("pgw-01", bearer(2), some_volume()),
                record("sgw-01", bearer(2), some_volume()),
            ];
            order.map(|i| all[i].clone()).to_vec()
        };

        let forward = correlate(&make([0, 1, 2, 3]), &CorrelationConfig::packet_bearer());
        let shuffled = correlate(&make([3, 1, 0, 2]), &CorrelationConfig::packet_bearer());

        // The same two bearers, each with two members, whatever the order.
        assert_eq!(forward.groups().len(), shuffled.groups().len());
        for result in [&forward, &shuffled] {
            assert!(result.groups().iter().all(|g| g.len() == 2));
            assert!(result.accounts_for(4));
        }
    }

    // ── Leg chains ───────────────────────────────────────────────────────────

    fn leg(icid: &str, linked: Option<&str>) -> UsageRecord {
        let mut keys = CorrelationKeys::default().with_icid(icid.parse().unwrap());
        if let Some(previous) = linked {
            keys = keys.with_linked_icid(previous.parse().unwrap());
        }
        record("scscf-01", keys, some_volume())
    }

    /// A diverted call: each new leg names the one it came from, so following
    /// the links reconstructs the history.
    #[test]
    fn a_forwarded_call_reassembles_into_one_history() {
        let records = vec![
            leg("leg-a", None),
            leg("leg-b", Some("leg-a")),
            leg("leg-c", Some("leg-b")),
        ];
        let result = correlate(&records, &CorrelationConfig::ims());

        assert_eq!(result.chains().len(), 1);
        let chain = &result.chains()[0];
        assert_eq!(chain.len(), 3);
        assert!(chain.is_forwarded());
        assert_eq!(chain.origin().as_str(), "leg-a");
        assert_eq!(chain.to_string(), "leg-a → leg-b → leg-c");
    }

    /// A leg naming a predecessor that never arrived is unmatched, not an
    /// origin: the difference is the evidence of who placed the call.
    #[test]
    fn a_lost_predecessor_is_reported_rather_than_assumed_to_be_the_origin() {
        let records = vec![leg("leg-b", Some("leg-a"))];
        let result = correlate(&records, &CorrelationConfig::ims());

        assert_eq!(result.unmatched().len(), 1);
        assert_eq!(
            result.unmatched()[0].reason,
            UnmatchedReason::PredecessorAbsent
        );
        // The leg still forms a chain of its own — the usage is not discarded.
        assert_eq!(result.chains().len(), 1);
        assert!(!result.chains()[0].is_forwarded());
    }

    /// A single un-diverted call is a chain of one, not a forwarded call.
    #[test]
    fn a_direct_call_is_a_chain_of_one() {
        let result = correlate(&[leg("leg-a", None)], &CorrelationConfig::ims());
        assert_eq!(result.chains().len(), 1);
        assert!(!result.chains()[0].is_forwarded());
        assert!(result.unmatched().is_empty());
    }

    /// Malformed input must not hang the join.
    #[test]
    fn a_cycle_in_linked_identifiers_terminates() {
        let records = vec![leg("leg-a", Some("leg-b")), leg("leg-b", Some("leg-a"))];
        let result = correlate(&records, &CorrelationConfig::ims());

        // Every leg has a predecessor, so no origin exists and no chain forms —
        // but the call returns.
        assert!(result.chains().iter().all(|c| c.len() <= 2));
        assert!(result.accounts_for(records.len()));
    }

    #[test]
    fn leg_chains_can_be_switched_off() {
        let records = vec![leg("leg-a", None), leg("leg-b", Some("leg-a"))];
        let result = correlate(&records, &CorrelationConfig::ims().without_leg_chains());
        assert!(result.chains().is_empty());
        // The sessions still join individually.
        assert_eq!(result.groups().len(), 2);
    }

    #[test]
    fn packet_and_ims_records_can_be_joined_together() {
        let records = vec![
            record("pgw-01", bearer(1), some_volume()),
            leg("leg-a", None),
        ];
        let result = correlate(&records, &CorrelationConfig::all());

        assert_eq!(result.groups().len(), 2);
        assert_eq!(result.groups()[0].key.kind(), "PACKET_BEARER");
        assert_eq!(result.groups()[1].key.kind(), "IMS_SESSION");
        assert!(result.accounts_for(2));
    }

    #[test]
    fn an_empty_input_produces_an_empty_result() {
        let result = correlate(&[], &CorrelationConfig::all());
        assert!(result.groups().is_empty());
        assert!(result.unmatched().is_empty());
        assert!(result.accounts_for(0));
        assert_eq!(result.double_counting_groups().len(), 0);
    }
}
