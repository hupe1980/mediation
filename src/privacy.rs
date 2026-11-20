//! Reducing an identity to what a question actually needs.
//!
//! Mediation holds the most sensitive dataset a telco operates, and most of
//! what is done with it afterwards does not need the identity. A device-mix
//! report needs the model, not the handset. A country-traffic report needs the
//! country, not the subscriber.
//!
//! # Pseudonymised data is still personal data
//!
//! EDPB Guidelines 01/2025 are explicit about it, and it is the single most
//! consequential misunderstanding in this area: a pseudonym is reversible by
//! whoever holds the key, so the output remains within the GDPR's scope. The
//! guidelines describe a *pseudonymisation domain* — an environment where only
//! pseudonymised data is processed and nobody has the additional information
//! needed to re-identify.
//!
//! [`Pseudonym`] is therefore **not** named "anonymous", carries no method
//! suggesting it is, and its documentation says so at the point of use. Calling
//! it anonymous is how a dataset ends up outside a retention policy it was
//! always inside.
//!
//! # The key is yours
//!
//! This crate holds no key material and ships no hash. A pseudonym must be
//! **keyed** — an unkeyed digest of a 15-digit IMSI is reversed by trying all
//! of them, which is minutes of work — so the primitive is supplied through
//! [`Pseudonymiser`], exactly as the E.212 table and the EEA membership list
//! are.
//!
//! What the crate contributes is the part that is easy to get wrong: which
//! fields carry identity, and which reductions preserve analytical value
//! without it.
//!
//! ```rust
//! use mediation::{Imei, Imsi, reduce_to_country};
//!
//! let imsi: Imsi = "262021234567890".parse().unwrap();
//! let imei: Imei = "490154203237518".parse().unwrap();
//!
//! // A country-traffic report needs the country, not the subscriber.
//! assert_eq!(reduce_to_country(imsi).to_string(), "262");
//!
//! // A device-mix report needs the model, not the handset.
//! assert_eq!(imei.tac().to_string(), "49015420");
//! ```

use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::str::FromStr;

use crate::dialplan::Destination;
use crate::digits::DigitString;
use crate::error::ParseError;
use crate::identity::{Imeisv, Imsi, Msisdn, Pei, Supi, Tac};
use crate::plmn::{Mcc, MncWidths, PlmnId};

// ── Pseudonym ────────────────────────────────────────────────────────────────

/// A keyed, stable stand-in for an identity.
///
/// # This is still personal data
///
/// A pseudonym is reversible by whoever holds the key, so under GDPR Art. 4(5)
/// and EDPB Guidelines 01/2025 the output stays within the regulation's scope.
/// It reduces risk; it does not remove the data from the rules. A dataset of
/// these still needs a retention policy, a lawful basis and a deletion path.
///
/// True anonymisation means the identity cannot be recovered by anyone,
/// including the key holder — which is [`Reduction`]'s business, not this
/// type's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pseudonym([u8; 32]);

impl Pseudonym {
    /// A pseudonym from a keyed digest.
    ///
    /// Constructed only by a [`Pseudonymiser`], so a raw identity cannot be
    /// dressed up as one by accident.
    #[must_use]
    pub const fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// The digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Pseudonym {
    /// Writes lower-case hex, which is what a storage key wants.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for Pseudonym {
    type Err = ParseError;

    /// Exactly 64 lower-case hex characters — the one form
    /// [`Display`](fmt::Display) writes (finding R10).
    ///
    /// Bytes are checked before any slicing, so no input panics, and a sign,
    /// whitespace or upper case is refused rather than read into a second
    /// spelling of the same key. The input is withheld from the error: a
    /// pseudonym is still personal data.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::withheld("Pseudonym", "64 lower-case hex characters");
        let bytes = s.as_bytes();
        if bytes.len() != 64 {
            return Err(err());
        }
        let nibble = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        };
        let mut digest = [0u8; 32];
        for (slot, pair) in digest.iter_mut().zip(bytes.as_chunks::<2>().0) {
            *slot = (nibble(pair[0]).ok_or_else(err)? << 4) | nibble(pair[1]).ok_or_else(err)?;
        }
        Ok(Self(digest))
    }
}

/// Produces a keyed pseudonym from an identity's bytes.
///
/// Implement this over the keyed primitive you already use — an HMAC, a keyed
/// BLAKE3, a KDF. The crate holds no key material and ships no hash, for the
/// same reason it ships no E.212 table: it is not the right owner.
///
/// The primitive **must be keyed**. An unkeyed digest of a 15-digit IMSI is
/// reversed by enumerating the 10^15 possibilities, which is minutes on
/// commodity hardware — so an unkeyed implementation of this trait produces a
/// value that is not a pseudonym at all.
pub trait Pseudonymiser {
    /// The keyed digest of `input`.
    fn digest(&self, input: &[u8]) -> [u8; 32];

    /// The pseudonym for an identity's canonical string form.
    ///
    /// Canonical, so that one identity has one pseudonym however it was
    /// spelled on arrival — the same reason every type here has exactly one
    /// string.
    fn pseudonymise(&self, identity: &impl fmt::Display) -> Pseudonym {
        Pseudonym::from_digest(self.digest(identity.to_string().as_bytes()))
    }
}

// ── Reduction ────────────────────────────────────────────────────────────────

/// How much of an identity to keep.
///
/// Unlike a pseudonym, a reduction is **irreversible**: the discarded digits
/// are gone, and nobody — key holder included — can recover them. That is what
/// makes a reduced dataset genuinely narrower rather than merely obscured.
///
/// This enum **names** a policy; it does not perform one. The three reductions
/// take different inputs and return different types — an [`Mcc`], a [`PlmnId`]
/// and a [`Tac`] — so each has its own function.
///
/// The reductions themselves are [`reduce_to_country`], [`reduce_to_network`]
/// and [`reduce_equipment`], each named for what it yields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reduction {
    /// Keep the country only.
    ///
    /// An IMSI becomes its [`Mcc`]; an MSISDN keeps its leading digits. Enough
    /// for traffic-by-country reporting and for nothing else.
    CountryOnly,
    /// Keep the network only.
    ///
    /// An IMSI becomes its [`PlmnId`]. Enough for interconnect and partner
    /// reporting, and it needs the same assignment table as
    /// [`Imsi::plmn`](crate::Imsi::plmn) — see [`crate::plmn`].
    NetworkOnly,
    /// Keep the equipment model only.
    ///
    /// An IMEI becomes its [`Tac`]. Enough for a device-mix report; a
    /// Type Allocation Code identifies a model, not a handset.
    ModelOnly,
}

impl Reduction {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[Self::CountryOnly, Self::NetworkOnly, Self::ModelOnly];

    /// The canonical codes, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] = &["COUNTRY_ONLY", "NETWORK_ONLY", "MODEL_ONLY"];

    /// What the reduction applies to.
    ///
    /// A model reduction is meaningless for a subscription identity and a
    /// network reduction is meaningless for a handset, so a configuration that
    /// pairs one with the wrong identity can be caught before it runs.
    #[must_use]
    pub const fn applies_to_subscription(self) -> bool {
        matches!(self, Self::CountryOnly | Self::NetworkOnly)
    }

    /// `true` when the reduction needs an [`MncWidths`] resolver.
    #[must_use]
    pub const fn needs_assignment_table(self) -> bool {
        matches!(self, Self::NetworkOnly)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CountryOnly => "COUNTRY_ONLY",
            Self::NetworkOnly => "NETWORK_ONLY",
            Self::ModelOnly => "MODEL_ONLY",
        }
    }
}

impl std::str::FromStr for Reduction {
    type Err = crate::error::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .find(|r| r.as_str().eq_ignore_ascii_case(s.trim()))
            .copied()
            .ok_or_else(|| crate::error::ParseError::one_of("Reduction", s, Self::CODES))
    }
}

impl fmt::Display for Reduction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Truncation ───────────────────────────────────────────────────────────────

/// The leading digits of a number, with the rest discarded irreversibly.
///
/// Useful for country- or range-level reporting on an [`Msisdn`]. **How many
/// digits are safe is not a constant**: a prefix long enough to be interesting
/// is long enough to be a small population, and a small population is
/// identifiable. The caller chooses, and the choice belongs in a data
/// protection impact assessment rather than in a library default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NumberPrefix {
    digits: crate::digits::DigitString,
}

impl NumberPrefix {
    /// The first `keep` digits of a number.
    ///
    /// `None` for a zero-length prefix, and for one at least as long as the
    /// number — which would not be a reduction at all, and returning the whole
    /// number from a method named for truncation is how identifying data ends
    /// up in a report that believed it was reduced.
    #[must_use]
    pub fn of(msisdn: Msisdn, keep: usize) -> Option<Self> {
        if keep == 0 || keep >= msisdn.len() {
            return None;
        }
        Some(Self {
            digits: msisdn.digits().slice(0, keep),
        })
    }

    /// A number without its last `hide` digits.
    ///
    /// The reduction the German itemised bill uses with `hide = 3`
    /// ([`itemise_called`] is the ready-made form). `None` when `hide` is 0 or
    /// at least the whole number — no caller arithmetic, so no underflow on a
    /// short number.
    #[must_use]
    pub fn without_last(msisdn: Msisdn, hide: usize) -> Option<Self> {
        let keep = msisdn.len().checked_sub(hide)?;
        if hide == 0 {
            return None;
        }
        Self::of(msisdn, keep)
    }

    /// How many digits were kept.
    #[must_use]
    pub fn len(&self) -> usize {
        self.digits.len()
    }

    /// Always `false` — a prefix is never constructed empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl fmt::Display for NumberPrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.digits.fmt(f)
    }
}

impl FromStr for NumberPrefix {
    type Err = ParseError;

    /// The rendered form back: 1 to 14 decimal digits, nothing else — a prefix
    /// is strictly shorter than the longest MSISDN (finding R11). The input
    /// is withheld from the error, as for any traffic data.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::withheld("NumberPrefix", "1 to 14 decimal digits");
        if s.is_empty() || s.len() >= Msisdn::MAX_DIGITS || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(err());
        }
        let digits = DigitString::parse(s, Msisdn::MAX_DIGITS - 1).ok_or_else(err)?;
        Ok(Self { digits })
    }
}

// ── Itemised bill ────────────────────────────────────────────────────────────

/// How the subscriber chose to see destinations on the itemised bill.
///
/// § 11 Abs. 1 S. 3 TDDDG: *„Dabei entscheidet der Endnutzer, ob ihm die von
/// ihm gewählten Rufnummern ungekürzt oder unter Kürzung um die letzten drei
/// Ziffern mitgeteilt werden."* No statute sets a default, so neither does
/// this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Itemisation {
    /// Destinations in full.
    Full,
    /// Destinations shortened by their last three digits.
    Shortened,
}

impl Itemisation {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[Self::Full, Self::Shortened];

    /// The canonical codes, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] = &["FULL", "SHORTENED"];

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "FULL",
            Self::Shortened => "SHORTENED",
        }
    }
}

impl fmt::Display for Itemisation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Itemisation {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .find(|i| i.as_str().eq_ignore_ascii_case(s.trim()))
            .copied()
            .ok_or_else(|| ParseError::one_of("Itemisation", s, Self::CODES))
    }
}

/// A number with its last three digits hidden: `+4930123xxx`, or `xxx` for a
/// destination of three digits or fewer.
///
/// Irreversible — the hidden digits are not kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShortenedNumber {
    visible: DigitString,
    international: bool,
}

impl ShortenedNumber {
    /// How many trailing digits are hidden (§ 11 Abs. 1 S. 3 TDDDG).
    pub const HIDDEN: usize = 3;

    fn of(digits: DigitString, international: bool) -> Self {
        let keep = digits.len().saturating_sub(Self::HIDDEN);
        Self {
            visible: digits.prefix(keep),
            // Nothing visible, nothing to mark: `xxx` is one value, so the
            // string form round-trips.
            international: international && keep > 0,
        }
    }

    /// The digits still shown.
    #[must_use]
    pub const fn visible(&self) -> DigitString {
        self.visible
    }
}

impl fmt::Display for ShortenedNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.international {
            f.write_str("+")?;
        }
        write!(f, "{}xxx", self.visible)
    }
}

impl FromStr for ShortenedNumber {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || {
            ParseError::withheld(
                "ShortenedNumber",
                "digits followed by xxx, such as +4930123xxx",
            )
        };
        let t = s.trim();
        let body = t.strip_suffix("xxx").ok_or_else(err)?;
        let (international, body) = match body.strip_prefix('+') {
            Some(rest) if !rest.is_empty() => (true, rest),
            Some(_) => return Err(err()),
            None => (false, body),
        };
        if body.starts_with(['-', ' ']) {
            return Err(err());
        }
        let visible = DigitString::parse(body, Msisdn::MAX_DIGITS).ok_or_else(err)?;
        if body.len() != visible.len() {
            return Err(err());
        }
        Ok(Self {
            visible,
            international,
        })
    }
}

/// How one destination appears on the itemised bill.
///
/// A presentation value: it takes no [`UsageRecord`](crate::UsageRecord) and
/// produces no disposition, so itemising a batch can never change what it
/// bills or how it reconciles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemisedDestination {
    /// Shown in full.
    Full(Destination),
    /// Shown without its last three digits.
    Shortened(ShortenedNumber),
    /// Not shown: the connection must not be recognisable (§ 11 Abs. 5 TDDDG).
    ///
    /// The call is still billed; only its line on the itemisation is left out.
    Omitted,
}

/// Destinations that must never be recognisable on an itemised bill.
///
/// § 11 Abs. 5 TDDDG: the itemisation *„darf nicht Verbindungen zu Anschlüssen
/// erkennen lassen"* of counselling services — Telefonseelsorge, health advice,
/// the groups of § 203 Abs. 1 Nr. 4 and 5 StGB — that the Federal Network Agency
/// has put on its list. Operators retrieve the list and must re-query it every
/// quarter (§ 11 Abs. 6 TDDDG); it arrives here as data, like every other list
/// that changes.
///
/// Matching is on the normalised [`Destination`], so `0800 111 0 111`,
/// `+49 800 1110111` and `0049800-1110111` are one entry — normalise the list
/// with the same [`DialContext`](crate::DialContext) as the calls.
pub trait ProtectedDestinations {
    /// `true` when calls to `destination` must not appear on the itemisation.
    fn is_protected(&self, destination: &Destination) -> bool;
}

impl ProtectedDestinations for [Destination] {
    fn is_protected(&self, destination: &Destination) -> bool {
        self.contains(destination)
    }
}

impl<const N: usize> ProtectedDestinations for [Destination; N] {
    fn is_protected(&self, destination: &Destination) -> bool {
        self.contains(destination)
    }
}

impl ProtectedDestinations for Vec<Destination> {
    fn is_protected(&self, destination: &Destination) -> bool {
        self.contains(destination)
    }
}

impl ProtectedDestinations for BTreeSet<Destination> {
    fn is_protected(&self, destination: &Destination) -> bool {
        self.contains(destination)
    }
}

impl<S: std::hash::BuildHasher> ProtectedDestinations for HashSet<Destination, S> {
    fn is_protected(&self, destination: &Destination) -> bool {
        self.contains(destination)
    }
}

/// How a called destination appears on the itemised bill.
///
/// Omitted when `protected` lists it, whatever the subscriber chose; otherwise
/// in full or without its last three digits, as the subscriber chose. A
/// destination of three digits or fewer is hidden entirely when shortened.
///
/// ```rust
/// use mediation::{Destination, Itemisation, ItemisedDestination, itemise_called};
///
/// let protected: [Destination; 1] = ["+498001110111".parse()?];
/// let call: Destination = "+49301234567".parse()?;
///
/// let shown = itemise_called(&call, Itemisation::Shortened, &protected);
/// let ItemisedDestination::Shortened(number) = shown else { unreachable!() };
/// assert_eq!(number.to_string(), "+49301234xxx");
///
/// let seelsorge: Destination = "+498001110111".parse()?;
/// assert_eq!(itemise_called(&seelsorge, Itemisation::Full, &protected), ItemisedDestination::Omitted);
/// # Ok::<(), mediation::ParseError>(())
/// ```
#[must_use]
pub fn itemise_called(
    destination: &Destination,
    choice: Itemisation,
    protected: &(impl ProtectedDestinations + ?Sized),
) -> ItemisedDestination {
    if protected.is_protected(destination) {
        return ItemisedDestination::Omitted;
    }
    match choice {
        Itemisation::Full => ItemisedDestination::Full(*destination),
        Itemisation::Shortened => ItemisedDestination::Shortened(ShortenedNumber::of(
            destination.digits(),
            destination.as_number().is_some(),
        )),
    }
}

/// How a caller's number appears on the itemised bill of the line that received
/// and paid for the call: **always** shortened.
///
/// § 11 Abs. 4 TDDDG: where the holder pays for incoming calls, the calling
/// numbers may be shown *„nur unter Kürzung um die letzten drei Ziffern"*. The
/// subscriber's [`Itemisation`] choice does not apply, so this function does not
/// take one.
#[must_use]
pub fn itemise_calling(calling: Msisdn) -> ShortenedNumber {
    ShortenedNumber::of(calling.digits(), true)
}

// ── Reduced subscriber ───────────────────────────────────────────────────────

/// A subscription reduced to a pseudonym, with the analytical dimensions kept.
///
/// The shape a warehouse wants: the identity is a pseudonym, and the country
/// and network — which carry no individual identity but do carry the reporting
/// value — survive as themselves.
///
/// # Still personal data
///
/// See [`Pseudonym`]. This type is narrower than a [`SubscriberRef`], not
/// outside the regulation.
///
/// [`SubscriberRef`]: crate::SubscriberRef
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReducedSubscriber {
    /// The keyed stand-in for the subscription identity.
    pub supi: Pseudonym,
    /// The country, where the identity was an IMSI.
    pub country: Option<Mcc>,
    /// The network, where an assignment table resolved it.
    pub network: Option<PlmnId>,
}

impl ReducedSubscriber {
    /// Reduce a subscription identity.
    ///
    /// The country is kept when the identity is an IMSI form; the network only
    /// when the supplied table resolves it. A network-specific identity yields
    /// neither, because it carries no country in its structure.
    #[must_use]
    pub fn of(supi: &Supi, pseudonymiser: &impl Pseudonymiser, widths: &impl MncWidths) -> Self {
        let imsi = supi.as_imsi();
        Self {
            supi: pseudonymiser.pseudonymise(supi),
            country: imsi.map(Imsi::mcc),
            network: imsi.and_then(|i| i.plmn(widths)),
        }
    }
}

impl fmt::Display for ReducedSubscriber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.supi)?;
        if let Some(network) = self.network {
            write!(f, " @{network}")?;
        } else if let Some(country) = self.country {
            write!(f, " @{country}")?;
        }
        Ok(())
    }
}

/// A subscription identity reduced to its country.
///
/// Needs no assignment table: the MCC is always the first three digits, which
/// is why this is the least an IMSI can be reduced to while remaining useful.
/// Many-to-one and irreversible — a country holds tens of millions of
/// subscribers.
#[must_use]
pub const fn reduce_to_country(imsi: Imsi) -> Mcc {
    imsi.mcc()
}

/// A subscription identity reduced to its network.
///
/// `None` when the resolver does not know the country — the same refusal to
/// guess as everywhere the MNC width is needed. See [`crate::plmn`].
#[must_use]
pub fn reduce_to_network(imsi: Imsi, widths: &impl MncWidths) -> Option<PlmnId> {
    imsi.plmn(widths)
}

/// Equipment reduced to its model.
///
/// Irreversible, and genuinely so: a Type Allocation Code names a model that
/// millions of handsets share.
#[must_use]
pub fn reduce_equipment(pei: Pei) -> Tac {
    pei.tac()
}

/// The software version an equipment identity carried, where it had one.
///
/// Separable from the model because a software-version report is a legitimate
/// question that needs no device identity at all.
#[must_use]
pub fn software_version(pei: Pei) -> Option<crate::identity::SoftwareVersion> {
    match pei {
        Pei::Imeisv(sv) => Some(Imeisv::software_version(sv)),
        Pei::Imei(_) => None,
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{
        Itemisation, NumberPrefix, Pseudonym, ReducedSubscriber, Reduction, ShortenedNumber,
    };
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for Pseudonym {
        /// Lower-case hex, which is what a storage key wants.
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_str(self)
        }
    }

    impl<'de> Deserialize<'de> for Pseudonym {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = Pseudonym;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("64 lower-case hex characters")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<Pseudonym, E> {
                    v.parse().map_err(de::Error::custom)
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    impl Serialize for Reduction {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for Reduction {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = Reduction;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a reduction code such as \"COUNTRY_ONLY\"")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<Reduction, E> {
                    Reduction::ALL
                        .iter()
                        .find(|r| r.as_str() == v)
                        .copied()
                        .ok_or_else(|| de::Error::custom(format!("unknown reduction {v:?}")))
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    impl Serialize for Itemisation {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for Itemisation {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = Itemisation;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("an itemisation code such as \"SHORTENED\"")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<Itemisation, E> {
                    v.parse().map_err(de::Error::custom)
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    impl Serialize for ShortenedNumber {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_str(self)
        }
    }

    impl<'de> Deserialize<'de> for ShortenedNumber {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = ShortenedNumber;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a shortened number such as \"+4930123xxx\"")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<ShortenedNumber, E> {
                    v.parse().map_err(de::Error::custom)
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    impl Serialize for NumberPrefix {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_str(self)
        }
    }

    impl<'de> Deserialize<'de> for NumberPrefix {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = NumberPrefix;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a number prefix such as \"49170\"")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<NumberPrefix, E> {
                    v.parse().map_err(de::Error::custom)
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "ReducedSubscriber")]
    struct Repr {
        supi: Pseudonym,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        country: Option<crate::plmn::Mcc>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        network: Option<crate::plmn::PlmnId>,
    }

    impl Serialize for ReducedSubscriber {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            Repr {
                supi: self.supi,
                country: self.country,
                network: self.network,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for ReducedSubscriber {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = Repr::deserialize(deserializer)?;
            Ok(ReducedSubscriber {
                supi: r.supi,
                country: r.country,
                network: r.network,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plmn::NorthAmericanRule;

    /// A deliberately trivial keyed digest, for tests only. A real one is an
    /// HMAC or a keyed BLAKE3 — see [`Pseudonymiser`].
    struct TestKey(u8);

    impl Pseudonymiser for TestKey {
        fn digest(&self, input: &[u8]) -> [u8; 32] {
            let mut out = [self.0; 32];
            for (index, byte) in input.iter().enumerate() {
                out[index % 32] ^= byte.rotate_left((index % 8) as u32);
            }
            out
        }
    }

    /// A pseudonym is stable for one identity and differs between keys — which
    /// is what makes a pseudonymisation domain a boundary rather than a label.
    #[test]
    fn a_pseudonym_is_stable_per_identity_and_per_key() {
        let supi: Supi = "imsi-262021234567890".parse().unwrap();
        let other: Supi = "imsi-262021234567891".parse().unwrap();

        let key = TestKey(1);
        assert_eq!(key.pseudonymise(&supi), key.pseudonymise(&supi));
        assert_ne!(key.pseudonymise(&supi), key.pseudonymise(&other));

        // A different key gives a different domain for the same subscriber.
        assert_ne!(key.pseudonymise(&supi), TestKey(2).pseudonymise(&supi));
    }

    /// One identity has one pseudonym however it was spelled on arrival,
    /// because the canonical string is what is hashed.
    #[test]
    fn spelling_does_not_change_a_pseudonym() {
        let key = TestKey(1);
        let imsi: Imsi = "262021234567890".parse().unwrap();
        let spaced: Imsi = "262 021 234 567 890".parse().unwrap();
        assert_eq!(key.pseudonymise(&imsi), key.pseudonymise(&spaced));
    }

    #[test]
    fn a_pseudonym_renders_and_parses_as_hex() {
        let key = TestKey(7);
        let supi: Supi = "imsi-262021234567890".parse().unwrap();
        let rendered = key.pseudonymise(&supi).to_string();
        assert_eq!(rendered.len(), 64);
        assert!(rendered.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── Reduction ────────────────────────────────────────────────────────────

    /// A reduction is irreversible, which is what makes it different in kind
    /// from a pseudonym.
    #[test]
    fn a_reduction_discards_rather_than_obscures() {
        let imsi: Imsi = "262021234567890".parse().unwrap();
        let country = reduce_to_country(imsi);
        assert_eq!(country.to_string(), "262");

        // Two subscribers in one country reduce to the same value, and nothing
        // recovers which was which.
        let other: Imsi = "262029999999999".parse().unwrap();
        assert_eq!(reduce_to_country(other), country);
    }

    #[test]
    fn a_network_reduction_needs_the_same_table_as_everything_else() {
        let imsi: Imsi = "262021234567890".parse().unwrap();
        assert_eq!(
            reduce_to_network(imsi, &NorthAmericanRule)
                .unwrap()
                .to_string(),
            "262-02"
        );

        let unknown = |_: Mcc| None;
        assert_eq!(
            reduce_to_network(imsi, &unknown),
            None,
            "an unresolvable network is not guessed"
        );
    }

    /// A Type Allocation Code names a model that millions of handsets share.
    #[test]
    fn equipment_reduces_to_a_model_from_either_form() {
        let imei: Pei = "imei-490154203237518".parse().unwrap();
        let imeisv: Pei = "imeisv-4901542032375120".parse().unwrap();

        assert_eq!(reduce_equipment(imei).to_string(), "49015420");
        assert_eq!(reduce_equipment(imei), reduce_equipment(imeisv));
        assert_eq!(reduce_equipment(imei), reduce_equipment(imei));
    }

    /// A software-version report needs no device identity at all.
    #[test]
    fn a_software_version_is_separable_from_the_device() {
        let imeisv: Pei = "imeisv-4901542032375120".parse().unwrap();
        assert_eq!(software_version(imeisv).unwrap().to_string(), "20");

        let imei: Pei = "imei-490154203237518".parse().unwrap();
        assert_eq!(software_version(imei), None, "an IMEI carries none");
    }

    // ── Truncation ───────────────────────────────────────────────────────────

    /// A "truncation" that keeps the whole number is not a reduction, and
    /// returning one from a method named for truncation is how identifying data
    /// reaches a report that believed it was reduced.
    #[test]
    fn a_prefix_must_actually_be_shorter() {
        let msisdn: Msisdn = "491701234567".parse().unwrap();

        assert_eq!(NumberPrefix::of(msisdn, 2).unwrap().to_string(), "49");
        assert_eq!(NumberPrefix::of(msisdn, 2).unwrap().len(), 2);

        assert_eq!(NumberPrefix::of(msisdn, 0), None, "nothing is not a prefix");
        assert_eq!(
            NumberPrefix::of(msisdn, msisdn.len()),
            None,
            "the whole number is not a reduction"
        );
        assert_eq!(NumberPrefix::of(msisdn, 99), None);
    }

    // ── Reduced subscriber ───────────────────────────────────────────────────

    #[test]
    fn a_reduced_subscriber_keeps_the_dimensions_and_loses_the_identity() {
        let supi: Supi = "imsi-262021234567890".parse().unwrap();
        let reduced = ReducedSubscriber::of(&supi, &TestKey(1), &NorthAmericanRule);

        assert_eq!(reduced.country, Some(Mcc::GERMANY));
        assert_eq!(reduced.network.unwrap().to_string(), "262-02");
        assert!(reduced.to_string().ends_with("@262-02"));

        // Two subscribers on one network share the dimensions and not the
        // pseudonym.
        let other: Supi = "imsi-262029999999999".parse().unwrap();
        let other = ReducedSubscriber::of(&other, &TestKey(1), &NorthAmericanRule);
        assert_eq!(reduced.network, other.network);
        assert_ne!(reduced.supi, other.supi);
    }

    /// A network-specific identity carries no country in its structure, so
    /// neither dimension is invented for it.
    #[test]
    fn a_network_specific_identity_yields_no_dimensions() {
        let supi: Supi = "nai-device-42@iot.example.org".parse().unwrap();
        let reduced = ReducedSubscriber::of(&supi, &TestKey(1), &NorthAmericanRule);

        assert_eq!(reduced.country, None);
        assert_eq!(reduced.network, None);
        assert_eq!(reduced.to_string(), reduced.supi.to_string());
    }

    #[test]
    fn reduction_codes_round_trip() {
        for reduction in Reduction::ALL {
            assert_eq!(reduction.to_string(), reduction.as_str());
        }
        assert_eq!(Reduction::ALL.len(), 3);
    }
}
