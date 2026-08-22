//! What the check digits do and do not catch.
//!
//! An IMEI carries a Luhn check digit and an IMEISV does not, because the
//! fifteenth position that holds the checksum in one is the first
//! software-version digit in the other. Running the check over an IMEISV
//! rejects valid equipment; not running it over an IMEI accepts corrupt
//! records. The types are separate so the compiler enforces the difference,
//! and this suite pins the behavioural consequences.

use mediation::{Iccid, Imei, Imeisv, Pei, Suci};
use proptest::prelude::*;

/// The IMEI used throughout 3GPP documentation and conformance material.
const REFERENCE_IMEI: &str = "490154203237518";

/// The same equipment with a software version of `20`.
const REFERENCE_IMEISV: &str = "4901542032375120";

#[test]
fn the_reference_identity_validates_and_decomposes() {
    let imei: Imei = REFERENCE_IMEI.parse().unwrap();
    assert_eq!(imei.tac().to_string(), "49015420");
    assert_eq!(imei.serial().to_string(), "323751");
    assert_eq!(imei.check_digit(), 8);
}

/// The point of a check digit: a single mistyped digit does not survive.
#[test]
fn every_single_digit_substitution_is_caught() {
    let digits: Vec<u8> = REFERENCE_IMEI.bytes().collect();

    let mut checked = 0;
    for position in 0..digits.len() {
        for replacement in b'0'..=b'9' {
            if replacement == digits[position] {
                continue;
            }
            let mut corrupted = digits.clone();
            corrupted[position] = replacement;
            let s = String::from_utf8(corrupted).unwrap();
            assert!(
                s.parse::<Imei>().is_err(),
                "single-digit error at {position} must be caught: {s}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 15 * 9);
}

/// Luhn catches every adjacent transposition **except** `09` against `90`.
///
/// A documented limitation of the algorithm, not of this implementation: the
/// two digits contribute the same weighted sum in either order. Pinned here so
/// nobody later mistakes it for a bug and "fixes" the checksum into something
/// non-standard.
#[test]
fn adjacent_transpositions_are_caught_except_the_known_luhn_blind_spot() {
    let digits: Vec<u8> = REFERENCE_IMEI.bytes().collect();

    let mut blind_spots = 0;
    for position in 0..digits.len() - 1 {
        let (a, b) = (digits[position], digits[position + 1]);
        if a == b {
            continue;
        }
        let mut swapped = digits.clone();
        swapped.swap(position, position + 1);
        let s = String::from_utf8(swapped).unwrap();

        let is_zero_nine_pair = matches!((a, b), (b'0', b'9') | (b'9', b'0'));
        if is_zero_nine_pair {
            assert!(
                s.parse::<Imei>().is_ok(),
                "the 0/9 pair is a known blind spot"
            );
            blind_spots += 1;
        } else {
            assert!(
                s.parse::<Imei>().is_err(),
                "transposition at {position} must be caught: {s}"
            );
        }
    }
    // The reference identity contains exactly one such pair, so the test is
    // exercising both branches rather than silently only one.
    assert_eq!(blind_spots, 1);
}

/// The bug the type split prevents, from both directions.
#[test]
fn an_imeisv_is_not_checksummed_and_an_imei_is() {
    let sv: Imeisv = REFERENCE_IMEISV.parse().unwrap();
    assert_eq!(sv.software_version().to_string(), "20");

    // Digit 15 is software-version data. Read as an IMEI it fails Luhn …
    assert!("490154203237512".parse::<Imei>().is_err());
    // … yet the IMEISV carrying it is perfectly valid.
    assert_eq!(sv.to_string(), REFERENCE_IMEISV);

    // And the derived IMEI is the checksummed identity of the same device.
    assert_eq!(sv.to_imei().to_string(), REFERENCE_IMEI);
    assert_eq!(sv.tac(), sv.to_imei().tac());
}

/// A record set mixing the two forms must not double-count handsets.
#[test]
fn both_equipment_forms_collapse_onto_one_device() {
    let as_imei: Pei = format!("imei-{REFERENCE_IMEI}").parse().unwrap();
    let as_imeisv: Pei = format!("imeisv-{REFERENCE_IMEISV}").parse().unwrap();

    assert_ne!(as_imei, as_imeisv, "the two records differ as written");
    assert_eq!(
        as_imei.to_imei(),
        as_imeisv.to_imei(),
        "…but name one device"
    );
    assert_eq!(as_imei.tac(), as_imeisv.tac());
}

/// Issuers ship 19- and 20-digit cards, so the checksum is reported rather than
/// enforced: rejecting a 19-digit ICCID for failing Luhn would reject valid SIMs.
#[test]
fn an_iccid_reports_its_checksum_rather_than_gatekeeping_on_it() {
    let nineteen: Iccid = "8949012345678901234".parse().unwrap();
    assert!(!nineteen.has_check_digit());
    assert_eq!(nineteen.len(), 19);

    let twenty: Iccid = "89490123456789012345".parse().unwrap();
    assert!(twenty.has_check_digit());
    assert_eq!(twenty.len(), 20);

    // The major industry identifier is enforced, because it is structural.
    assert!("1249012345678901234".parse::<Iccid>().is_err());
}

proptest! {
    /// Every derived IMEI validates: the computed check digit and the verifier
    /// are two sides of the same algorithm, and a mismatch between them would
    /// mean the crate emits identities it would itself reject.
    #[test]
    fn computed_check_digits_satisfy_the_verifier(body in "[0-9]{16}") {
        let sv: Imeisv = body.parse().unwrap();
        let derived = sv.to_imei().to_string();
        prop_assert_eq!(derived.len(), 15);
        prop_assert!(derived.parse::<Imei>().is_ok(), "{} must validate", derived);
        // The first 14 digits are carried through untouched.
        prop_assert_eq!(&derived[..14], &sv.to_string()[..14]);
    }

    /// Corrupting any digit of a valid IMEI is caught, whatever the identity.
    #[test]
    fn single_digit_errors_are_caught_for_any_identity(
        body in "[0-9]{16}",
        position in 0usize..15,
        delta in 1u8..=9,
    ) {
        let valid = body.parse::<Imeisv>().unwrap().to_imei().to_string();
        let mut digits: Vec<u8> = valid.bytes().collect();
        digits[position] = b'0' + (digits[position] - b'0' + delta) % 10;
        let corrupted = String::from_utf8(digits).unwrap();
        prop_assert!(
            corrupted.parse::<Imei>().is_err(),
            "corruption of {} at {} must be caught: {}",
            valid, position, corrupted
        );
    }
}

// ── SUCI ─────────────────────────────────────────────────────────────────────

/// TS 23.003 §2.2B fixes seven fields in one order. The routing indicator is
/// easy to leave out and the scheme and key identifiers are easy to swap — both
/// are small integers that look alike in a debug print — and a value spelled
/// either way round is rejected by every core it reaches, only at the point of
/// use.
#[test]
fn a_suci_spells_its_seven_fields_in_the_order_the_spec_fixes() {
    let canonical = "suci-0-262-02-0678-1-27-0072b7d4c1e8";
    let suci: Suci = canonical.parse().unwrap();

    assert_eq!(suci.home_network.to_string(), "262-02");
    assert_eq!(suci.routing_indicator.to_string(), "0678");
    assert_eq!(suci.protection_scheme, 1);
    assert_eq!(suci.public_key_id, 27);
    assert_eq!(suci.scheme_output, "0072b7d4c1e8");
    assert_eq!(suci.to_string(), canonical);
    assert!(!suci.is_null_scheme());
}

/// The routing indicator keeps its leading zeros, exactly as an MNC does: `0`
/// and `0000` route to different places.
#[test]
fn a_routing_indicator_keeps_its_leading_zeros() {
    let short: Suci = "suci-0-262-02-678-0-0-1234567890".parse().unwrap();
    let padded: Suci = "suci-0-262-02-0678-0-0-1234567890".parse().unwrap();

    assert_ne!(short, padded);
    assert_eq!(short.routing_indicator.to_string(), "678");
    assert_eq!(padded.routing_indicator.to_string(), "0678");
    assert!(short.is_null_scheme());
}

#[test]
fn a_suci_refuses_what_it_cannot_spell_back() {
    for s in [
        "",
        "suci-0-262-02-0678-1-27",       // the pre-fix six-field spelling
        "suci-1-262-02-0678-1-27-00ab",  // the NAI form is a different shape
        "suci-0-262-02-06789-1-27-00ab", // five routing digits
        "suci-0-262-02-0678-16-27-00ab", // the scheme identifier is four bits
        "suci-0-262-02-0678-1-256-00ab", // the key identifier is one octet
        "suci-0-262-02-0678-1-27-",      // no scheme output
        "suci-0-262-2-0678-1-27-00ab",   // an MNC keeps its leading zero
    ] {
        assert!(s.parse::<Suci>().is_err(), "{s:?} must not parse");
    }
}
