//! Deduplication: the natural key is the whole design.
//!
//! Duplicates are not an edge case. They are the normal consequence of a
//! retransmission after a lost acknowledgement, a re-sent file after a
//! partner's failed load, and a node's own recovery replay. Getting this wrong
//! bills the same usage twice.
//!
//! # Why hashing the record fails
//!
//! A retransmitted record is frequently **not** byte-identical to the original:
//! node identifiers, padding, extension fields and file-level context differ.
//! A whole-record hash therefore lets retransmissions straight through — the
//! exact failure the mechanism exists to prevent.
//!
//! # Why keying on subscriber and start time also fails
//!
//! It over-deduplicates. **Concurrent sessions on one subscription are
//! normal**: a 5G handset holds several simultaneous PDU sessions across
//! different data networks and slices, and two of them can be established in
//! the same second. Keying on subscriber and timestamp silently discards one —
//! under-billing, and invisible.
//!
//! # The natural key comes from the standard
//!
//! | Domain | Key |
//! |---|---|
//! | Packet data, 5G and LTE | Charging ID **and gateway address**, plus the partial-record sequence |
//! | IMS | ICID, the recording entity, and the partial-record sequence |
//! | Anything with a recording-entity serial | the **recording entity** and its `localSequenceNumber` |
//!
//! 3GPP TS 32.251 is explicit that a Charging ID is unique only together with
//! the P-GW address, which is why [`crate::CorrelationKeys::packet_session`]
//! yields the pair or nothing.
//!
//! # The recording entity is not the collecting node
//!
//! A `localSequenceNumber` is unique within the element that **created** the
//! record, and a gateway routinely collects records from several elements into
//! one file. Keying on the file's node would give one record two different keys
//! when it is collected through two gateways — so every retransmission would
//! pass, which is the failure this whole module exists to prevent.
//!
//! [`crate::CorrelationKeys::recording_entity`] therefore carries the creating
//! element, and a serial number without it is [`NotKeyable`] rather than
//! silently scoped to the wrong thing.
//!
//! # A record that cannot be keyed cannot be deduplicated
//!
//! [`RecordKey::of`] returns a [`NotKeyable`] rather than falling back to
//! provenance. A provenance-derived key — node, file, offset — is unique per
//! *arrival*, which is the opposite of what deduplication needs: it is
//! different for every copy of the same record, so every retransmission would
//! pass. Saying so is the only safe answer.
//!
//! ```rust
//! use mediation::{NotKeyable, RecordKey};
//! # use mediation::{ChargedParty, ChargingId, CorrelationKeys, RecordType, SourceRef,
//! #                 SubscriberRef, UsageRecord};
//! # use std::net::Ipv4Addr;
//! # let party = ChargedParty::Subscriber(
//! #     SubscriberRef::new("imsi-262021234567890".parse().unwrap()));
//! # let source = SourceRef::new("pgw-01".parse().unwrap(), 0);
//! # let opened = "2026-07-28T10:00:00+02:00".parse().unwrap();
//! let bare = UsageRecord::new(RecordType::PduSession, party.clone(), opened, source.clone());
//! assert_eq!(RecordKey::of(&bare), Err(NotKeyable::NoCorrelatorPresent));
//!
//! let keyed = bare.with_correlation(
//!     CorrelationKeys::default()
//!         .with_charging_id(ChargingId::new(1_234))
//!         .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()),
//! );
//! assert!(RecordKey::of(&keyed).is_ok());
//! ```
//!
//! # The window is a promise about the outside world
//!
//! An unbounded store is not a design, it is a memory leak, so the window is
//! mandatory. It encodes "our partners never resend later than this" — and if
//! that is false, duplicates pass and revenue is billed twice.
//! [`DedupWindow::from_bound`] is named for the fact it records rather than for
//! the buffer it sizes.

use std::collections::HashMap;
use std::fmt;

use time::{Duration, OffsetDateTime};

use crate::correlation::{Icid, PacketSessionId};
use crate::record::{NodeId, UsageRecord};

// ── RecordKey ────────────────────────────────────────────────────────────────

/// The identity a record keeps across retransmission.
///
/// Derived from the record's own domain fields, never from a random value and
/// never from where it arrived: the key must be **stable across
/// retransmission**, which is a different property from uniqueness and the
/// reason a generated identifier would be wrong here.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecordKey {
    /// A packet-data record, keyed on the session and its partial sequence.
    PacketSession {
        /// Charging ID together with the gateway that assigned it.
        session: PacketSessionId,
        /// The partial-record sequence, absent on a record that is whole.
        sequence: Option<u32>,
    },
    /// An IMS record, keyed on the session and its partial sequence.
    ///
    /// The ICID is generated once, by the first element to handle the session,
    /// and is globally unique — so unlike a `localSequenceNumber` it needs no
    /// entity to scope it, and adding one would split a session's records
    /// across the elements that reported them.
    Ims {
        /// The IMS session identifier.
        icid: Icid,
        /// The partial-record sequence, absent on a record that is whole.
        sequence: Option<u32>,
    },
    /// Any record carrying a serial number unique within its recording entity.
    RecordingEntity {
        /// The element that **created** the record and assigned the serial —
        /// not the node whose file it arrived in.
        entity: NodeId,
        /// The serial number, unique within that entity.
        local_sequence: u32,
    },
}

impl RecordKey {
    /// The natural key of a record, or why it has none.
    ///
    /// Prefers the most specific correlator the record carries. The order is
    /// stable and documented so that two copies of one record — which may
    /// carry different subsets of the correlators — cannot key differently:
    ///
    /// 1. packet session (Charging ID **and** gateway)
    /// 2. IMS session (the globally unique ICID)
    /// 3. recording entity **and** its `localSequenceNumber`
    ///
    /// # Errors
    ///
    /// [`NotKeyable`] when the record carries none of the above. Such a record
    /// must be dispositioned rather than deduplicated — see the module
    /// documentation.
    pub fn of(record: &UsageRecord) -> Result<Self, NotKeyable> {
        let keys = &record.correlation;

        if let Some(session) = keys.packet_session() {
            return Ok(Self::PacketSession {
                session,
                sequence: keys.record_sequence_number,
            });
        }
        if keys.charging_id.is_some() != keys.gateway.is_some() {
            // Half a packet-session identity is worse than none: it looks
            // usable and identifies the wrong thing.
            return Err(NotKeyable::IncompletePacketSessionId);
        }
        if let Some(icid) = &keys.icid {
            return Ok(Self::Ims {
                icid: icid.clone(),
                sequence: keys.record_sequence_number,
            });
        }
        if let Some((entity, local_sequence)) = keys.entity_serial() {
            return Ok(Self::RecordingEntity {
                entity: entity.clone(),
                local_sequence,
            });
        }
        if keys.local_sequence_number.is_some() || keys.recording_entity.is_some() {
            // Half an entity-scoped identity. A serial number scoped to the
            // wrong element, or an element with no serial, identifies nothing.
            return Err(NotKeyable::IncompleteEntitySerial);
        }
        Err(NotKeyable::NoCorrelatorPresent)
    }

    /// A short code naming the key's shape, for grouping and reporting.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::PacketSession { .. } => "PACKET_SESSION",
            Self::Ims { .. } => "IMS",
            Self::RecordingEntity { .. } => "RECORDING_ENTITY",
        }
    }
}

impl fmt::Display for RecordKey {
    /// Writes `KIND:parts`, compact enough for a duplicate-report line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PacketSession { session, sequence } => {
                write!(f, "PACKET_SESSION:{session}")?;
                if let Some(sequence) = sequence {
                    write!(f, "/{sequence}")?;
                }
                Ok(())
            }
            Self::Ims { icid, sequence } => {
                write!(f, "IMS:{icid}")?;
                if let Some(sequence) = sequence {
                    write!(f, "/{sequence}")?;
                }
                Ok(())
            }
            Self::RecordingEntity {
                entity,
                local_sequence,
            } => write!(f, "RECORDING_ENTITY:{entity}/{local_sequence}"),
        }
    }
}

/// Why a record has no natural key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NotKeyable {
    /// The record carried no correlator this crate can key on.
    NoCorrelatorPresent,
    /// A Charging ID without its gateway, or a gateway without its Charging ID.
    ///
    /// Distinguished from [`NoCorrelatorPresent`](Self::NoCorrelatorPresent)
    /// because it points at a decoder dropping a field rather than at a record
    /// type that never carries one — a different fix, in a different place.
    IncompletePacketSessionId,
    /// A `localSequenceNumber` with no recording entity, or the reverse.
    ///
    /// The serial is unique only within the element that assigned it, so one
    /// half of the pair identifies nothing.
    IncompleteEntitySerial,
}

impl NotKeyable {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[
        Self::NoCorrelatorPresent,
        Self::IncompletePacketSessionId,
        Self::IncompleteEntitySerial,
    ];

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoCorrelatorPresent => "NO_CORRELATOR_PRESENT",
            Self::IncompletePacketSessionId => "INCOMPLETE_PACKET_SESSION_ID",
            Self::IncompleteEntitySerial => "INCOMPLETE_ENTITY_SERIAL",
        }
    }
}

impl fmt::Display for NotKeyable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for NotKeyable {}

// ── DedupWindow ──────────────────────────────────────────────────────────────

/// How far back the deduplication store remembers.
///
/// A promise about partner behaviour, not a buffer size: a duplicate arriving
/// after the window closes will **not** be caught. The crate documents that
/// consequence rather than offering an unbounded alternative, because an
/// unbounded store is a memory leak with a correctness story attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DedupWindow(Duration);

impl DedupWindow {
    /// A window from the longest retransmission delay a partner may exhibit.
    ///
    /// `None` for a non-positive duration: a zero window catches nothing while
    /// appearing to be deduplication.
    #[must_use]
    pub const fn from_bound(bound: Duration) -> Option<Self> {
        if bound.is_positive() {
            Some(Self(bound))
        } else {
            None
        }
    }

    /// The window's length.
    #[must_use]
    pub const fn get(self) -> Duration {
        self.0
    }

    /// The oldest observation instant still inside the window at `as_of`.
    #[must_use]
    pub fn cutoff(self, as_of: OffsetDateTime) -> OffsetDateTime {
        as_of - self.0
    }
}

// ── DedupStore ───────────────────────────────────────────────────────────────

/// What happened when a key was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupDecision {
    /// Not seen before within the window. The record proceeds.
    First,
    /// Seen before, at the carried instant. The record is suppressed.
    Duplicate {
        /// When the surviving copy was first observed.
        first_seen: OffsetDateTime,
    },
}

impl DedupDecision {
    /// `true` when this observation is the one that proceeds.
    #[must_use]
    pub const fn is_first(self) -> bool {
        matches!(self, Self::First)
    }
}

/// Remembers which keys have been seen, within a bounded window.
///
/// **Caller-owned.** The crate holds no state of its own, so a run is a pure
/// mapping from `(input, store, as_of)` to `(outcome, store′)` — which is what
/// makes a replay reproducible.
///
/// Not automatically evicting: [`evict`](Self::evict) is called by the caller,
/// at a cadence they choose, and reports how many keys it dropped. Automatic
/// eviction inside `observe` would make the store's size depend on arrival
/// order, and therefore on how the caller batched their input.
#[derive(Debug, Clone)]
pub struct DedupStore {
    window: DedupWindow,
    seen: HashMap<RecordKey, OffsetDateTime>,
}

impl DedupStore {
    /// An empty store bound to a window.
    ///
    /// The window is held by the store rather than passed to each eviction, so
    /// two calls cannot disagree about how long the promise runs — a store
    /// evicted against a shorter window than it was filled under silently loses
    /// duplicate detection for the difference.
    #[must_use]
    pub fn new(window: DedupWindow) -> Self {
        Self {
            window,
            seen: HashMap::new(),
        }
    }

    /// An empty store with room for `capacity` keys.
    #[must_use]
    pub fn with_capacity(window: DedupWindow, capacity: usize) -> Self {
        Self {
            window,
            seen: HashMap::with_capacity(capacity),
        }
    }

    /// The window this store remembers within.
    #[must_use]
    pub const fn window(&self) -> DedupWindow {
        self.window
    }

    /// Record an observation of `key` at `at`.
    ///
    /// Idempotent: observing the same key repeatedly yields
    /// [`DedupDecision::First`] exactly once, then
    /// [`DedupDecision::Duplicate`] every time after — never twice, never zero
    /// times. The first-seen instant is retained rather than refreshed, so a
    /// stream of retransmissions cannot keep a key alive past its window.
    pub fn observe(&mut self, key: RecordKey, at: OffsetDateTime) -> DedupDecision {
        match self.seen.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => DedupDecision::Duplicate {
                first_seen: *entry.get(),
            },
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(at);
                DedupDecision::First
            }
        }
    }

    /// Drop keys first seen before the window's cutoff, returning how many.
    ///
    /// After this, a duplicate of a dropped key will be classified as
    /// [`DedupDecision::First`] and will be billed again. That is the window's
    /// stated consequence, not a defect — but it is why the bound must reflect
    /// real partner behaviour.
    pub fn evict(&mut self, as_of: OffsetDateTime) -> usize {
        let cutoff = self.window.cutoff(as_of);
        let before = self.seen.len();
        self.seen.retain(|_, first_seen| *first_seen >= cutoff);
        before - self.seen.len()
    }

    /// When a key was first seen, if it is still remembered.
    #[must_use]
    pub fn first_seen(&self, key: &RecordKey) -> Option<OffsetDateTime> {
        self.seen.get(key).copied()
    }

    /// How many keys are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// `true` when nothing is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.seen.clear();
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{NotKeyable, RecordKey};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;
    use std::str::FromStr;

    impl Serialize for NotKeyable {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for NotKeyable {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = NotKeyable;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a not-keyable reason code")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<NotKeyable, E> {
                    NotKeyable::ALL
                        .iter()
                        .find(|r| r.as_str() == v)
                        .copied()
                        .ok_or_else(|| de::Error::custom(format!("unknown reason {v:?}")))
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    /// An internally tagged representation: the `kind` discriminator is stable
    /// and readable, and adding a key shape does not renumber the others.
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename = "RecordKey")]
    enum RecordKeyRepr {
        #[serde(rename = "PACKET_SESSION")]
        PacketSession {
            session: crate::correlation::PacketSessionId,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            sequence: Option<u32>,
        },
        #[serde(rename = "IMS")]
        Ims {
            icid: crate::correlation::Icid,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            sequence: Option<u32>,
        },
        #[serde(rename = "RECORDING_ENTITY")]
        RecordingEntity {
            entity: crate::record::NodeId,
            local_sequence: u32,
        },
    }

    impl Serialize for RecordKey {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let repr = match self {
                Self::PacketSession { session, sequence } => RecordKeyRepr::PacketSession {
                    session: *session,
                    sequence: *sequence,
                },
                Self::Ims { icid, sequence } => RecordKeyRepr::Ims {
                    icid: icid.clone(),
                    sequence: *sequence,
                },
                Self::RecordingEntity {
                    entity,
                    local_sequence,
                } => RecordKeyRepr::RecordingEntity {
                    entity: entity.clone(),
                    local_sequence: *local_sequence,
                },
            };
            repr.serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for RecordKey {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Ok(match RecordKeyRepr::deserialize(deserializer)? {
                RecordKeyRepr::PacketSession { session, sequence } => {
                    Self::PacketSession { session, sequence }
                }
                RecordKeyRepr::Ims { icid, sequence } => Self::Ims { icid, sequence },
                RecordKeyRepr::RecordingEntity {
                    entity,
                    local_sequence,
                } => Self::RecordingEntity {
                    entity,
                    local_sequence,
                },
            })
        }
    }

    // Silence an unused-import warning when the feature is on but FromStr is
    // reached only through the visitors above.
    const _: fn(&str) -> Result<crate::correlation::Icid, crate::ParseError> =
        crate::correlation::Icid::from_str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::correlation::{ChargingId, CorrelationKeys};
    use crate::party::{ChargedParty, SubscriberRef};
    use crate::record::{RecordType, SourceRef};
    use std::net::Ipv4Addr;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-07-28 00:00:00 UTC);

    fn record(node: &str, correlation: CorrelationKeys) -> UsageRecord {
        UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new(node.parse().unwrap(), 0),
        )
        .with_correlation(correlation)
    }

    fn packet_keys(charging_id: u32, gateway: [u8; 4]) -> CorrelationKeys {
        CorrelationKeys::default()
            .with_charging_id(ChargingId::new(charging_id))
            .with_gateway(Ipv4Addr::from(gateway).into())
    }

    // ── Key derivation ───────────────────────────────────────────────────────

    /// The confirmed 3GPP rule: a Charging ID is unique only with its gateway,
    /// so two gateways using the same counter are two different sessions.
    #[test]
    fn the_gateway_is_part_of_the_packet_session_key() {
        let a = RecordKey::of(&record("pgw-01", packet_keys(1_234, [10, 0, 0, 1]))).unwrap();
        let b = RecordKey::of(&record("pgw-02", packet_keys(1_234, [10, 0, 0, 2]))).unwrap();
        assert_ne!(a, b, "same Charging ID, different gateways, different keys");

        // …and the node the record arrived through is *not* part of it, so the
        // same session collected twice keys identically.
        let same = RecordKey::of(&record("cgf-99", packet_keys(1_234, [10, 0, 0, 1]))).unwrap();
        assert_eq!(a, same);
    }

    /// The bug this returns an error for: half an identity looks usable and
    /// identifies the wrong thing.
    #[test]
    fn half_a_packet_session_identity_is_an_error_not_a_key() {
        let only_id = record(
            "pgw-01",
            CorrelationKeys::default().with_charging_id(ChargingId::new(1)),
        );
        assert_eq!(
            RecordKey::of(&only_id),
            Err(NotKeyable::IncompletePacketSessionId)
        );

        let only_gateway = record(
            "pgw-01",
            CorrelationKeys::default().with_gateway(Ipv4Addr::LOCALHOST.into()),
        );
        assert_eq!(
            RecordKey::of(&only_gateway),
            Err(NotKeyable::IncompletePacketSessionId)
        );
    }

    /// A record with nothing to key on must say so, not fall back to
    /// provenance — a provenance key differs for every copy, so every
    /// retransmission would pass.
    #[test]
    fn a_record_without_correlators_has_no_key() {
        let bare = record("pgw-01", CorrelationKeys::none());
        assert_eq!(RecordKey::of(&bare), Err(NotKeyable::NoCorrelatorPresent));
    }

    #[test]
    fn partials_of_one_session_are_distinct_keys() {
        let first = record(
            "pgw-01",
            packet_keys(7, [10, 0, 0, 1]).with_record_sequence(1),
        );
        let second = record(
            "pgw-01",
            packet_keys(7, [10, 0, 0, 1]).with_record_sequence(2),
        );
        let whole = record("pgw-01", packet_keys(7, [10, 0, 0, 1]));

        let (a, b, c) = (
            RecordKey::of(&first).unwrap(),
            RecordKey::of(&second).unwrap(),
            RecordKey::of(&whole).unwrap(),
        );
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    /// A serial number scoped to its recording entity.
    fn entity_keys(entity: &str, serial: u32) -> CorrelationKeys {
        CorrelationKeys::default()
            .with_recording_entity(entity.parse().unwrap())
            .with_local_sequence(serial)
    }

    #[test]
    fn ims_and_recording_entity_keys_are_derived_in_order() {
        let ims = record(
            "cgf-01",
            entity_keys("scscf-01", 5).with_icid("icid-abc".parse().unwrap()),
        );
        // The ICID is globally unique, so it wins over the entity serial.
        assert!(matches!(
            RecordKey::of(&ims).unwrap(),
            RecordKey::Ims { .. }
        ));

        let serial = record("cgf-01", entity_keys("msc-01", 5));
        assert_eq!(
            RecordKey::of(&serial).unwrap(),
            RecordKey::RecordingEntity {
                entity: "msc-01".parse().unwrap(),
                local_sequence: 5,
            }
        );
    }

    /// A `localSequenceNumber` is unique only within the element that assigned
    /// it, so that element is part of the key.
    #[test]
    fn a_local_serial_is_scoped_to_its_recording_entity() {
        let a = record("cgf-01", entity_keys("msc-01", 5));
        let b = record("cgf-01", entity_keys("msc-02", 5));
        assert_ne!(RecordKey::of(&a).unwrap(), RecordKey::of(&b).unwrap());
    }

    /// The bug this key shape fixes: one record collected through two gateways
    /// must key identically, or every retransmission passes.
    #[test]
    fn the_collecting_node_is_not_part_of_any_key() {
        assert_eq!(
            RecordKey::of(&record("cgf-01", entity_keys("msc-01", 5))).unwrap(),
            RecordKey::of(&record("cgf-02", entity_keys("msc-01", 5))).unwrap(),
        );

        let ims = CorrelationKeys::default().with_icid("icid-abc".parse().unwrap());
        assert_eq!(
            RecordKey::of(&record("cgf-01", ims.clone())).unwrap(),
            RecordKey::of(&record("cgf-02", ims)).unwrap(),
        );
    }

    /// Half an entity-scoped identity identifies nothing, and says so.
    #[test]
    fn half_an_entity_serial_is_an_error_not_a_key() {
        for keys in [
            CorrelationKeys::default().with_local_sequence(5),
            CorrelationKeys::default().with_recording_entity("msc-01".parse().unwrap()),
        ] {
            assert_eq!(
                RecordKey::of(&record("cgf-01", keys)),
                Err(NotKeyable::IncompleteEntitySerial)
            );
        }
    }

    // ── Store ────────────────────────────────────────────────────────────────

    fn hour_window() -> DedupWindow {
        DedupWindow::from_bound(Duration::hours(1)).expect("positive")
    }

    #[test]
    fn observing_a_key_repeatedly_yields_one_first_and_then_duplicates() {
        let mut store = DedupStore::new(hour_window());
        let key = RecordKey::of(&record("pgw-01", packet_keys(1, [10, 0, 0, 1]))).unwrap();

        assert_eq!(store.observe(key.clone(), NOW), DedupDecision::First);
        for i in 1..5 {
            assert_eq!(
                store.observe(key.clone(), NOW + Duration::minutes(i)),
                DedupDecision::Duplicate { first_seen: NOW },
                "observation {i} must be a duplicate"
            );
        }
        assert_eq!(store.len(), 1);
    }

    /// The first-seen instant is retained rather than refreshed, so a stream of
    /// retransmissions cannot keep a key alive past its window.
    #[test]
    fn retransmissions_do_not_extend_a_keys_life() {
        let mut store = DedupStore::new(hour_window());
        let key = RecordKey::of(&record("pgw-01", packet_keys(1, [10, 0, 0, 1]))).unwrap();

        store.observe(key.clone(), NOW);
        store.observe(key.clone(), NOW + Duration::minutes(59));
        assert_eq!(store.first_seen(&key), Some(NOW));

        // Ninety minutes on, the key is older than the window despite the
        // recent retransmission.
        assert_eq!(store.evict(NOW + Duration::minutes(90)), 1);
        assert!(store.is_empty());
    }

    /// The window's stated consequence, asserted rather than left implicit.
    #[test]
    fn a_duplicate_arriving_after_the_window_is_not_caught() {
        let mut store = DedupStore::new(hour_window());
        let key = RecordKey::of(&record("pgw-01", packet_keys(1, [10, 0, 0, 1]))).unwrap();

        assert_eq!(store.observe(key.clone(), NOW), DedupDecision::First);
        store.evict(NOW + Duration::hours(2));
        assert_eq!(
            store.observe(key, NOW + Duration::hours(2)),
            DedupDecision::First,
            "past the window the store has forgotten, and says so by design"
        );
    }

    #[test]
    fn eviction_keeps_keys_inside_the_window() {
        let mut store = DedupStore::new(hour_window());

        for (i, at) in [NOW - Duration::hours(3), NOW - Duration::minutes(30), NOW]
            .into_iter()
            .enumerate()
        {
            let key = RecordKey::of(&record(
                "pgw-01",
                packet_keys(u32::try_from(i).unwrap(), [10, 0, 0, 1]),
            ))
            .unwrap();
            store.observe(key, at);
        }
        assert_eq!(store.len(), 3);
        assert_eq!(store.evict(NOW), 1);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn a_zero_or_negative_window_is_rejected() {
        assert!(DedupWindow::from_bound(Duration::ZERO).is_none());
        assert!(DedupWindow::from_bound(Duration::seconds(-1)).is_none());
        assert!(DedupWindow::from_bound(Duration::seconds(1)).is_some());
    }

    #[test]
    fn keys_render_legibly() {
        let packet = RecordKey::of(&record(
            "pgw-01",
            packet_keys(1_234, [10, 0, 0, 1]).with_record_sequence(2),
        ))
        .unwrap();
        assert_eq!(packet.to_string(), "PACKET_SESSION:1234@10.0.0.1/2");
        assert_eq!(packet.kind(), "PACKET_SESSION");

        let serial = RecordKey::of(&record(
            "cgf-01",
            CorrelationKeys::default()
                .with_recording_entity("msc-01".parse().unwrap())
                .with_local_sequence(9),
        ))
        .unwrap();
        assert_eq!(serial.to_string(), "RECORDING_ENTITY:msc-01/9");
    }
}
