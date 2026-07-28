//! Walking a record file as bytes.
//!
//! A 3GPP charging record is ASN.1 (X.680) transferred in BER (X.690). This
//! crate reads the **framing** — tags, lengths, nesting, bounds — and not the
//! schema, because the schema is thousands of generated lines that belong
//! beside a generator.
//!
//! Framing alone is enough to walk a file, find a field by its tag path, check
//! that a record's declared length matches what arrived, and reject a truncated
//! one. This suite builds a record shaped like a real one and does exactly
//! that, then goes after the input a hostile sender would produce.

use mediation::{BerElement, BerReader, BerTag, CdrTimestamp, Century, DigitString, TagClass};

/// Build a context-specific TLV, using the high-tag-number form past 30.
///
/// The encoder is spelled out here rather than reused from the crate, so the
/// suite tests the reader against an independently written encoder rather than
/// against itself.
fn context(number: u32, constructed: bool, content: &[u8]) -> Vec<u8> {
    let class_and_form = 0x80 | (u8::from(constructed) << 5);
    let mut out = Vec::new();

    if number < 31 {
        out.push(class_and_form | u8::try_from(number).unwrap());
    } else {
        out.push(class_and_form | 0x1F);
        // Base 128, most significant first, continuation bit on all but the last.
        let mut septets = Vec::new();
        let mut remaining = number;
        while remaining > 0 {
            septets.push(u8::try_from(remaining & 0x7F).unwrap());
            remaining >>= 7;
        }
        septets.reverse();
        let last = septets.len() - 1;
        for (index, septet) in septets.iter().enumerate() {
            out.push(if index == last {
                *septet
            } else {
                septet | 0x80
            });
        }
    }

    let len = content.len();
    if len < 128 {
        out.push(u8::try_from(len).unwrap());
    } else {
        // Two-octet long form is enough for anything this suite builds.
        out.push(0x82);
        out.push(u8::try_from(len >> 8).unwrap());
        out.push(u8::try_from(len & 0xFF).unwrap());
    }
    out.extend_from_slice(content);
    out
}

/// A record shaped like a packet-data CDR: an IMSI in TBCD, a nine-octet
/// timestamp, and a nested list of two service-data containers.
fn packet_record() -> Vec<u8> {
    let imsi: DigitString = "262021234567890".parse().unwrap();
    let mut imsi_wire = [0u8; 8];
    let n = imsi.to_tbcd(&mut imsi_wire).unwrap();

    let opened: CdrTimestamp = "2026-07-28T10:00:00+02:00".parse().unwrap();

    let container = |rating_group: u8, octets: u32| -> Vec<u8> {
        context(
            1,
            true,
            &[
                context(0, false, &integer_content(u64::from(rating_group))),
                context(1, false, &integer_content(u64::from(octets))),
            ]
            .concat(),
        )
    };

    context(
        79, // beyond the short tag form, so the high-tag path is exercised too
        true,
        &[
            context(3, false, &imsi_wire[..n]),
            context(13, false, &opened.to_bcd().unwrap()),
            context(
                34,
                true,
                &[container(10, 4_096), container(20, 65_535)].concat(),
            ),
        ]
        .concat(),
    )
}

/// The minimal BER contents for a non-negative integer (X.690 §8.3).
///
/// A leading zero octet is required when the top bit of the first significant
/// octet is set, or the value would read as negative — which is exactly what
/// `BerElement::as_u64` refuses to guess at.
fn integer_content(value: u64) -> Vec<u8> {
    if value == 0 {
        return vec![0x00];
    }
    let mut octets: Vec<u8> = value
        .to_be_bytes()
        .into_iter()
        .skip_while(|b| *b == 0)
        .collect();
    if octets[0] & 0x80 != 0 {
        octets.insert(0, 0x00);
    }
    octets
}

fn ctx(number: u32, constructed: bool) -> BerTag {
    BerTag::new(TagClass::ContextSpecific, constructed, number).unwrap()
}

fn only(bytes: &[u8]) -> BerElement<'_> {
    let mut reader = BerReader::new(bytes);
    let element = reader.next().expect("one element").expect("well formed");
    assert!(reader.next().is_none(), "exactly one element");
    element
}

// ── Walking a record ─────────────────────────────────────────────────────────

/// Find each field by its tag and decode it with the codecs the crate already
/// has — which is the whole point of separating framing from schema.
#[test]
fn a_record_can_be_walked_field_by_field() {
    let bytes = packet_record();
    let record = only(&bytes);

    assert_eq!(record.tag().number(), 79);
    assert!(record.tag().is_constructed());

    // The IMSI field: TBCD, per TS 29.002.
    let imsi_field = record.find(ctx(3, false)).unwrap().expect("an IMSI field");
    let imsi = mediation::Imsi::from_tbcd(imsi_field.content()).unwrap();
    assert_eq!(imsi.to_string(), "262021234567890");

    // The timestamp field: plain BCD, per TS 32.298 — the other convention.
    let time_field = record.find(ctx(13, false)).unwrap().expect("a timestamp");
    let opened = CdrTimestamp::from_bcd(time_field.content(), Century::TWENTY_FIRST).unwrap();
    assert_eq!(opened.to_string(), "2026-07-28T10:00:00+02:00");

    // The container list, one level deeper.
    let containers = record
        .find(ctx(34, true))
        .unwrap()
        .expect("a container list");
    let members: Vec<_> = containers
        .children()
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(members.len(), 2);

    let volumes: Vec<u64> = members
        .iter()
        .map(|c| c.find(ctx(1, false)).unwrap().unwrap().as_u64().unwrap())
        .collect();
    assert_eq!(volumes, vec![4_096, 65_535]);
}

/// Offsets point into the file the caller holds, at every depth — otherwise a
/// finding cannot be taken back to the bytes that produced it.
#[test]
fn offsets_point_into_the_original_file_at_every_depth() {
    let bytes = packet_record();
    let record = only(&bytes);
    assert_eq!(record.offset(), 0);

    let imsi_field = record.find(ctx(3, false)).unwrap().unwrap();
    // The field really begins at the offset it reports.
    assert_eq!(bytes[imsi_field.offset()], 0x83, "a primitive context 3");

    let containers = record.find(ctx(34, true)).unwrap().unwrap();
    for member in containers.children().unwrap() {
        let member = member.unwrap();
        assert_eq!(bytes[member.offset()], 0xA1, "a constructed context 1");
        let volume = member.find(ctx(1, false)).unwrap().unwrap();
        assert_eq!(bytes[volume.offset()], 0x81);
    }
}

/// `find` searches one level, deliberately: a tag number means something only
/// at the level its schema places it.
#[test]
fn find_does_not_reach_into_nested_structures() {
    let bytes = packet_record();
    let record = only(&bytes);

    // Container tag 1 exists two levels down; the record must not return it.
    assert!(record.find(ctx(1, false)).unwrap().is_none());
    // …while the list that holds it is right there.
    assert!(record.find(ctx(34, true)).unwrap().is_some());
}

#[test]
fn a_whole_record_validates_within_a_depth_bound() {
    let bytes = packet_record();
    let record = only(&bytes);

    // Record → container list → container → field is three levels below the
    // record itself.
    let visited = record.validate(4).unwrap();
    assert_eq!(visited, 10, "the record, 3 fields, 2 containers, 4 leaves");
    assert!(record.validate(2).is_err(), "a shallow bound refuses");
}

/// Zero copy: every element's contents are a view into the caller's buffer.
#[test]
fn nothing_is_copied() {
    let bytes = packet_record();
    let record = only(&bytes);

    let field = record.find(ctx(3, false)).unwrap().unwrap();
    let start = field.content().as_ptr() as usize - bytes.as_ptr() as usize;
    assert_eq!(
        &bytes[start..start + field.content().len()],
        field.content()
    );
}

// ── Hostile input ────────────────────────────────────────────────────────────

/// A truncated file is reported, never partially decoded into a record missing
/// exactly the fields that failed.
#[test]
fn a_truncated_record_is_rejected_rather_than_half_read() {
    let full = packet_record();
    for cut in 1..full.len() {
        let truncated = &full[..cut];
        let outcome: Result<Vec<_>, _> = BerReader::new(truncated).collect();
        match outcome {
            Err(_) => {}
            Ok(elements) => {
                // A prefix may parse cleanly only if it ends on an element
                // boundary — in which case the record is simply shorter.
                let consumed: usize = elements.iter().map(BerElement::encoded_len).sum();
                assert_eq!(
                    consumed,
                    truncated.len(),
                    "cut at {cut} decoded past its end"
                );
            }
        }
    }
}

/// Deep nesting is attacker-controlled. The bound is mandatory, and exceeding
/// it is reported rather than crashing.
#[test]
fn pathological_nesting_is_bounded_not_fatal() {
    // Nest as deep as the one-octet length form allows, then check the walk
    // refuses a shallow bound and survives a generous one.
    let mut bytes = vec![0x05u8, 0x00];
    let mut depth = 0;
    while bytes.len() + 2 < 128 {
        let len = u8::try_from(bytes.len()).unwrap();
        let mut wrapped = vec![0xA0, len];
        wrapped.extend_from_slice(&bytes);
        bytes = wrapped;
        depth += 1;
    }
    assert!(depth > 50, "deep enough to matter, got {depth}");

    let element = only(&bytes);
    assert!(element.validate(depth + 1).is_ok());
    assert!(element.validate(3).is_err());
}

/// A length claiming more than the file holds must not be believed.
#[test]
fn a_length_beyond_the_input_is_rejected() {
    // Claims 0x7FFF octets of contents in a six-octet buffer.
    let bytes = [0x04, 0x82, 0x7F, 0xFF, 0x00, 0x00];
    assert!(BerReader::new(&bytes).next().unwrap().is_err());
}

/// Two encodings of one value would give one record two hashes and two
/// deduplication keys.
#[test]
fn non_minimal_encodings_are_refused() {
    // A long-form length that fits the short form.
    assert!(
        BerReader::new(&[0x04, 0x81, 0x02, 0xAA, 0xBB])
            .next()
            .unwrap()
            .is_err()
    );
    // A high-tag form encoding a number the short form covers.
    assert!(BerReader::new(&[0xBF, 0x0A, 0x00]).next().unwrap().is_err());
    // A padded integer.
    let padded = only(&[0x02, 0x02, 0x00, 0x2A]);
    assert!(padded.as_u64().is_err());
}

/// The indefinite form is legal for constructed elements and must survive a
/// zero pair appearing inside contents.
#[test]
fn indefinite_lengths_are_delimited_by_structure_not_by_a_byte_search() {
    let inner = context(1, false, &[0x00, 0x00]);
    let mut bytes = vec![0xA0, 0x80];
    bytes.extend_from_slice(&inner);
    bytes.extend_from_slice(&[0x00, 0x00]);

    let element = only(&bytes);
    assert_eq!(element.content(), inner.as_slice());
    assert_eq!(element.encoded_len(), bytes.len());

    let child = element.children().unwrap().next().unwrap().unwrap();
    assert_eq!(child.content(), &[0x00, 0x00]);
}

/// Every byte of a well-formed record is accounted for: the sum of the
/// top-level elements' encoded lengths is the file.
#[test]
fn a_record_accounts_for_every_octet() {
    let one = packet_record();
    let two = [one.clone(), one.clone()].concat();

    let elements: Vec<_> = BerReader::new(&two).collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(elements.len(), 2);
    let consumed: usize = elements.iter().map(BerElement::encoded_len).sum();
    assert_eq!(consumed, two.len());
    assert_eq!(elements[1].offset(), one.len());
}

/// The encoder in this file and the reader in the crate agree on the minimal
/// integer form, including where a leading zero is required.
#[test]
fn integer_encoding_round_trips_through_the_reader() {
    for value in [
        0u64,
        1,
        127,
        128,
        255,
        256,
        4_096,
        65_535,
        u64::from(u32::MAX),
    ] {
        let bytes = context(2, false, &integer_content(value));
        assert_eq!(only(&bytes).as_u64().unwrap(), value, "value {value}");
    }
}
