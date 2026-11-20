//! Basic Encoding Rules — the framing every 3GPP charging record is wrapped in.
//!
//! 3GPP TS 32.298 defines its records in ASN.1 (X.680) and transfers them in
//! BER (X.690). This module reads the **framing**: tags, lengths, nesting and
//! bounds. It does not know what any particular tag means, because that is the
//! schema, and a schema is thousands of generated lines that belong beside a
//! generator rather than here.
//!
//! Framing alone is enough to walk a record file, find a field by its tag path,
//! check that a record's declared length matches what arrived, and reject a
//! truncated one — which is most of what a mediation layer needs from the bytes.
//!
//! # Zero copy
//!
//! [`BerElement::content`] borrows from the input. Decoding a 200 MB record file
//! copies nothing, which matters at the volumes this crate is built for.
//!
//! ```rust
//! use mediation::{BerReader, TagClass};
//!
//! // A constructed context-specific 0 holding an INTEGER 42 and an INTEGER 7.
//! let bytes = [0xA0, 0x06, 0x02, 0x01, 0x2A, 0x02, 0x01, 0x07];
//!
//! let outer = BerReader::new(&bytes).next().unwrap().unwrap();
//! assert_eq!(outer.tag().class(), TagClass::ContextSpecific);
//! assert_eq!(outer.tag().number(), 0);
//! assert!(outer.tag().is_constructed());
//!
//! let inner: Vec<_> = outer.children().unwrap().collect::<Result<_, _>>().unwrap();
//! assert_eq!(inner.len(), 2);
//! assert_eq!(inner[0].content(), &[0x2A]);
//! // Offsets point into the original input, not into the parent's contents.
//! assert_eq!(inner[0].offset(), 2);
//! ```
//!
//! # What BER permits, and what it does not
//!
//! BER leaves the length form to the sender: a definite length may use the
//! long form even when the short form would do, with any number of length
//! octets (X.690 §8.1.3.5, NOTE 2). Such lengths are **accepted** — a
//! conforming sender may produce them. DER (X.690 §10.1) is the stricter
//! subset that forbids them; this module reads BER.
//!
//! The encodings X.690 forbids even in BER are **rejected**: a high-tag-number
//! form with a leading `0x80` octet (§8.1.2.4.2 c), the high-tag form for a
//! number below 31 (§8.1.2.2), the reserved length octet `0xFF` (§8.1.3.5 c),
//! and an INTEGER with redundant leading octets (§8.3.2). A raw-byte hash of a
//! BER record is therefore not a canonical key; hash decoded fields instead.
//!
//! # Depth is bounded
//!
//! Nesting is attacker-controlled in a record that arrived over a network.
//! [`BerElement::validate`] walks a structure to a caller-supplied depth and
//! refuses to go deeper, so a pathological file cannot exhaust the stack. The
//! iterators themselves never recurse.

use std::fmt;

use crate::error::DecodeError;

// ── Tag ──────────────────────────────────────────────────────────────────────

/// The class of a BER tag (X.690 §8.1.2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TagClass {
    /// Types defined by X.680 itself — `INTEGER`, `OCTET STRING`, `SEQUENCE`.
    Universal,
    /// Types defined by an application, which is where 3GPP's records sit.
    Application,
    /// Tags whose meaning depends on where they appear.
    ///
    /// The overwhelming majority of a charging record: its fields are
    /// context-specific and numbered by position in the ASN.1 definition.
    ContextSpecific,
    /// Private-use tags.
    Private,
}

impl TagClass {
    /// Every variant, in the order X.690 numbers them.
    pub const ALL: &'static [Self] = &[
        Self::Universal,
        Self::Application,
        Self::ContextSpecific,
        Self::Private,
    ];

    /// The class encoded in the top two bits of an identifier octet.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        match bits >> 6 {
            0b00 => Self::Universal,
            0b01 => Self::Application,
            0b10 => Self::ContextSpecific,
            _ => Self::Private,
        }
    }

    /// The top two bits of an identifier octet for this class.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Universal => 0b0000_0000,
            Self::Application => 0b0100_0000,
            Self::ContextSpecific => 0b1000_0000,
            Self::Private => 0b1100_0000,
        }
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Universal => "UNIVERSAL",
            Self::Application => "APPLICATION",
            Self::ContextSpecific => "CONTEXT",
            Self::Private => "PRIVATE",
        }
    }
}

impl fmt::Display for TagClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A decoded BER tag: class, constructed flag and number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BerTag {
    class: TagClass,
    constructed: bool,
    number: u32,
}

impl BerTag {
    /// The largest tag number this reader accepts.
    ///
    /// A 32-bit ceiling: X.690 places no limit, but a tag beyond this is a
    /// misaligned read rather than a field, and accepting an unbounded integer
    /// from untrusted input is how a decoder becomes a denial of service.
    pub const MAX_NUMBER: u32 = u32::MAX >> 1;

    /// A tag from its three parts.
    ///
    /// `None` for a number above [`MAX_NUMBER`](Self::MAX_NUMBER).
    #[must_use]
    pub const fn new(class: TagClass, constructed: bool, number: u32) -> Option<Self> {
        if number > Self::MAX_NUMBER {
            return None;
        }
        Some(Self {
            class,
            constructed,
            number,
        })
    }

    /// The tag's class.
    #[must_use]
    pub const fn class(self) -> TagClass {
        self.class
    }

    /// `true` when the contents are further elements rather than raw bytes.
    #[must_use]
    pub const fn is_constructed(self) -> bool {
        self.constructed
    }

    /// The tag number.
    #[must_use]
    pub const fn number(self) -> u32 {
        self.number
    }

    /// `true` when this is the universal end-of-contents marker.
    #[must_use]
    pub const fn is_end_of_contents(self) -> bool {
        matches!(self.class, TagClass::Universal) && !self.constructed && self.number == 0
    }
}

impl fmt::Display for BerTag {
    /// Writes `CONTEXT 3` or `CONTEXT 3 (constructed)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.class, self.number)?;
        if self.constructed {
            f.write_str(" (constructed)")?;
        }
        Ok(())
    }
}

// ── Length ───────────────────────────────────────────────────────────────────

/// How a BER element's contents are delimited (X.690 §8.1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BerLength {
    /// A stated octet count.
    Definite(usize),
    /// Terminated by an end-of-contents marker, valid only for constructed
    /// elements.
    Indefinite,
}

impl BerLength {
    /// The stated octet count, when there is one.
    #[must_use]
    pub const fn definite(self) -> Option<usize> {
        match self {
            Self::Definite(n) => Some(n),
            Self::Indefinite => None,
        }
    }
}

// ── Element ──────────────────────────────────────────────────────────────────

/// One tag-length-value element, borrowing its contents from the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BerElement<'a> {
    tag: BerTag,
    length: BerLength,
    content: &'a [u8],
    /// Octets this element occupies in the input, header and contents together.
    encoded_len: usize,
    /// Where the element began, for error reporting against the original input.
    offset: usize,
    /// Where the **contents** began, so a child's reported offset points into
    /// the original input rather than into its parent.
    content_offset: usize,
}

impl<'a> BerElement<'a> {
    /// The element's tag.
    #[must_use]
    pub const fn tag(&self) -> BerTag {
        self.tag
    }

    /// How the contents were delimited.
    #[must_use]
    pub const fn length(&self) -> BerLength {
        self.length
    }

    /// The contents, borrowed from the input.
    ///
    /// For a constructed element these are the encoded children; use
    /// [`children`](Self::children) to walk them.
    #[must_use]
    pub const fn content(&self) -> &'a [u8] {
        self.content
    }

    /// How many octets the whole element occupies, header included.
    #[must_use]
    pub const fn encoded_len(&self) -> usize {
        self.encoded_len
    }

    /// Where the element began in the input it was read from.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// A reader over this element's children.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when the element is primitive: its contents are raw
    /// bytes, and reading them as elements would produce fields that are not
    /// there.
    pub fn children(&self) -> Result<BerReader<'a>, DecodeError> {
        if !self.tag.constructed {
            return Err(DecodeError::new(
                self.offset,
                "a constructed element; a primitive one has no children",
            ));
        }
        Ok(BerReader::at_offset(self.content, self.content_offset))
    }

    /// The contents as an unsigned integer.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for an empty field, one wider than eight octets, a
    /// negative value, or a non-minimal encoding — X.690 §8.3 requires the
    /// shortest form, and a padded integer is a second encoding of one value.
    pub fn as_u64(&self) -> Result<u64, DecodeError> {
        let bytes = self.content;
        if bytes.is_empty() {
            return Err(DecodeError::new(self.offset, "at least one content octet"));
        }
        if bytes[0] & 0x80 != 0 {
            return Err(DecodeError::new(self.offset, "a non-negative integer"));
        }
        if bytes.len() > 1 && bytes[0] == 0x00 && bytes[1] & 0x80 == 0 {
            return Err(DecodeError::new(
                self.offset,
                "a minimally encoded integer, without leading padding",
            ));
        }
        // Nine octets are the widest a `u64` can hold: X.690 integers are
        // signed, so `u64::MAX` is encoded as a `0x00` sign octet followed by
        // eight value octets. Rejecting that form would put the top bit of the
        // range out of reach for no reason.
        let value = if bytes[0] == 0x00 { &bytes[1..] } else { bytes };
        if value.len() > 8 {
            return Err(DecodeError::new(
                self.offset,
                "an integer that fits in 64 unsigned bits",
            ));
        }
        Ok(value.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b)))
    }

    /// Walk this element's structure, refusing to descend past `max_depth`.
    ///
    /// Returns how many elements were visited, this one included.
    ///
    /// Nesting is attacker-controlled in a record that arrived over a network,
    /// so the bound is mandatory rather than optional. The walk is iterative
    /// and never recurses, so exceeding the bound is reported rather than
    /// crashing.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when the structure is malformed, or nests deeper than
    /// `max_depth`.
    pub fn validate(&self, max_depth: usize) -> Result<usize, DecodeError> {
        let mut visited = 1;
        // (contents, offset, depth) — an explicit stack, so the recursion depth
        // of this function is one whatever the input does.
        let mut pending = Vec::new();
        if self.tag.constructed {
            pending.push((self.content, self.content_offset, 1usize));
        }

        while let Some((bytes, offset, depth)) = pending.pop() {
            if depth > max_depth {
                return Err(DecodeError::new(
                    offset,
                    "a structure within the depth limit",
                ));
            }
            for element in BerReader::at_offset(bytes, offset) {
                let element = element?;
                visited += 1;
                if element.tag.constructed {
                    pending.push((element.content, element.content_offset, depth + 1));
                }
            }
        }
        Ok(visited)
    }

    /// The first child whose tag matches, searching only this element's own
    /// children.
    ///
    /// Not recursive: a tag number is only meaningful at the level its schema
    /// places it, and a deep search would happily return a field from an
    /// unrelated nested structure.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when the element is primitive or its children are
    /// malformed.
    pub fn find(&self, tag: BerTag) -> Result<Option<Self>, DecodeError> {
        for child in self.children()? {
            let child = child?;
            if child.tag == tag {
                return Ok(Some(child));
            }
        }
        Ok(None)
    }
}

impl fmt::Display for BerElement<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at {}, {} content octet(s)",
            self.tag,
            self.offset,
            self.content.len()
        )
    }
}

// ── Reader ───────────────────────────────────────────────────────────────────

/// An iterator over the BER elements at one nesting level.
///
/// Yields `Result`, so a malformed element stops the walk rather than being
/// skipped — a decoder that steps over what it could not read produces a record
/// missing the fields it failed on, which is worse than no record.
#[derive(Debug, Clone)]
pub struct BerReader<'a> {
    bytes: &'a [u8],
    position: usize,
    /// Offset of `bytes[0]` in the original input, so errors and elements
    /// report positions a caller can find in the file they hold.
    base: usize,
    finished: bool,
}

impl<'a> BerReader<'a> {
    /// How deep indefinite-length constructions may nest.
    ///
    /// X.690 sets no limit, and a real charging record nests a handful of
    /// levels. The bound exists because resolving an indefinite length means
    /// walking the element's subtree, so an unbounded chain of them is
    /// quadratic in the file size — and because the input is untrusted.
    pub const MAX_INDEFINITE_DEPTH: usize = 128;

    /// A reader over `bytes`.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self::at_offset(bytes, 0)
    }

    /// A reader over `bytes`, which begin at `base` in the original input.
    #[must_use]
    pub const fn at_offset(bytes: &'a [u8], base: usize) -> Self {
        Self {
            bytes,
            position: 0,
            base,
            finished: false,
        }
    }

    /// How many octets remain unread.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    /// Read one element, or `None` at the end of the input.
    fn read(&mut self) -> Option<Result<BerElement<'a>, DecodeError>> {
        if self.finished || self.position >= self.bytes.len() {
            return None;
        }
        // Every reported offset is `base + i` with `i <= bytes.len()`; one
        // check here keeps all of them from overflowing.
        if self.base.checked_add(self.bytes.len()).is_none() {
            self.finished = true;
            return Some(Err(DecodeError::new(
                self.base,
                "a base offset that leaves room for the input",
            )));
        }
        let start = self.position;
        match self.read_element(start) {
            Ok(element) => {
                self.position += element.encoded_len;
                Some(Ok(element))
            }
            Err(e) => {
                self.finished = true;
                Some(Err(e))
            }
        }
    }

    /// Read the element beginning at `start`.
    ///
    /// An end-of-contents marker is an **error** here rather than a quiet stop.
    /// A reader only ever walks a region whose extent is already known — an
    /// element's contents, or the caller's whole input — so a marker inside one
    /// closes nothing. Treating it as a terminator would silently drop every
    /// remaining field of a definite-length structure, which is exactly the
    /// "decoder that steps over what it could not read" this module refuses to
    /// be. The scan that resolves an indefinite length recognises the marker by
    /// inspection, before reaching here.
    fn read_element(&self, start: usize) -> Result<BerElement<'a>, DecodeError> {
        let absolute = self.base + start;
        let (tag, after_tag) = self.read_tag(start)?;
        let (length, after_length) = self.read_length(after_tag)?;

        if tag.is_end_of_contents() {
            return Err(DecodeError::new(
                absolute,
                "an element; an end-of-contents marker closes nothing here",
            ));
        }

        let (content, encoded_len) = match length {
            BerLength::Definite(n) => {
                let end = after_length
                    .checked_add(n)
                    .ok_or_else(|| DecodeError::new(absolute, "a length that does not overflow"))?;
                if end > self.bytes.len() {
                    return Err(DecodeError::new(
                        absolute,
                        "contents that fit within the input",
                    ));
                }
                (&self.bytes[after_length..end], end - start)
            }
            BerLength::Indefinite => {
                if !tag.constructed {
                    return Err(DecodeError::new(
                        absolute,
                        "a definite length; only constructed elements may be indefinite",
                    ));
                }
                let content_end = self.scan_to_end_of_contents(after_length)?;
                // The contents stop before the two-octet marker, but the
                // element occupies them too.
                (
                    &self.bytes[after_length..content_end],
                    content_end + 2 - start,
                )
            }
        };

        Ok(BerElement {
            tag,
            length,
            content,
            encoded_len,
            offset: absolute,
            content_offset: self.base + after_length,
        })
    }

    /// Decode the identifier octets (X.690 §8.1.2).
    fn read_tag(&self, start: usize) -> Result<(BerTag, usize), DecodeError> {
        let absolute = self.base + start;
        let first = *self
            .bytes
            .get(start)
            .ok_or_else(|| DecodeError::new(absolute, "an identifier octet"))?;
        let class = TagClass::from_bits(first);
        let constructed = first & 0b0010_0000 != 0;
        let low = first & 0b0001_1111;

        if low != 0b0001_1111 {
            let tag = BerTag::new(class, constructed, u32::from(low))
                .ok_or_else(|| DecodeError::new(absolute, "a tag number within range"))?;
            return Ok((tag, start + 1));
        }

        // High-tag-number form: base-128, most significant first, bit 8 set on
        // every octet but the last.
        let mut number = 0u32;
        let mut index = start + 1;
        let mut octets = 0usize;
        loop {
            let byte = *self
                .bytes
                .get(index)
                .ok_or_else(|| DecodeError::new(absolute, "a complete high-tag-number form"))?;
            if octets == 0 && byte == 0x80 {
                return Err(DecodeError::new(
                    absolute,
                    "a minimally encoded tag number, without leading padding",
                ));
            }
            number = number
                .checked_mul(128)
                .and_then(|n| n.checked_add(u32::from(byte & 0x7F)))
                .ok_or_else(|| DecodeError::new(absolute, "a tag number within range"))?;
            index += 1;
            octets += 1;
            if byte & 0x80 == 0 {
                break;
            }
        }
        if number < 31 {
            return Err(DecodeError::new(
                absolute,
                "the short tag form for numbers below 31",
            ));
        }
        let tag = BerTag::new(class, constructed, number)
            .ok_or_else(|| DecodeError::new(absolute, "a tag number within range"))?;
        Ok((tag, index))
    }

    /// Decode the length octets (X.690 §8.1.3).
    fn read_length(&self, start: usize) -> Result<(BerLength, usize), DecodeError> {
        let absolute = self.base + start;
        let first = *self
            .bytes
            .get(start)
            .ok_or_else(|| DecodeError::new(absolute, "a length octet"))?;

        if first & 0x80 == 0 {
            return Ok((BerLength::Definite(usize::from(first)), start + 1));
        }
        let count = usize::from(first & 0x7F);
        if count == 0 {
            return Ok((BerLength::Indefinite, start + 1));
        }
        if count == 0x7F {
            return Err(DecodeError::new(absolute, "a length form X.690 defines"));
        }
        let end = start + 1 + count;
        if end > self.bytes.len() {
            return Err(DecodeError::new(absolute, "complete length octets"));
        }
        // BER lets the sender choose the long form and pad it with leading
        // zero octets (X.690 §8.1.3.5); only the value matters.
        let octets = &self.bytes[start + 1..end];
        let significant = octets
            .iter()
            .position(|b| *b != 0)
            .map_or(&octets[octets.len()..], |i| &octets[i..]);
        if significant.len() > 8 {
            return Err(DecodeError::new(absolute, "a length that fits in 64 bits"));
        }
        let value = significant
            .iter()
            .fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
        let value = usize::try_from(value)
            .map_err(|_| DecodeError::new(absolute, "a length this platform can address"))?;
        Ok((BerLength::Definite(value), end))
    }

    /// Find the end-of-contents marker closing an indefinite-length element.
    ///
    /// Walks the nested elements rather than scanning for `00 00`, because two
    /// zero octets occur inside contents all the time.
    ///
    /// **Iterative, and depth-bounded.** Nesting is attacker-controlled in a
    /// record that arrived over a network, so a scan that recursed once per
    /// level would abort the process on a file that is a few hundred kilobytes
    /// of `30 80` — no allocation and no cleverness required of the attacker.
    /// This walk keeps a counter instead, and refuses input nested deeper than
    /// [`MAX_INDEFINITE_DEPTH`](Self::MAX_INDEFINITE_DEPTH). The bound also caps
    /// the cost of descending: resolving an indefinite length is inherently a
    /// walk of the element's whole subtree, so an unbounded chain of them would
    /// be quadratic in the file size even without the stack.
    fn scan_to_end_of_contents(&self, start: usize) -> Result<usize, DecodeError> {
        let mut position = start;
        let mut open = 1usize;
        loop {
            if position + 2 > self.bytes.len() {
                return Err(DecodeError::new(
                    self.base + start,
                    "an end-of-contents marker",
                ));
            }
            if self.bytes[position] == 0x00 && self.bytes[position + 1] == 0x00 {
                open -= 1;
                if open == 0 {
                    return Ok(position);
                }
                position += 2;
                continue;
            }

            // Read the header only. A definite-length element is stepped over
            // wholesale; an indefinite one opens another level of this loop.
            let (tag, after_tag) = self.read_tag(position)?;
            let (length, after_length) = self.read_length(after_tag)?;
            if tag.is_end_of_contents() {
                return Err(DecodeError::new(
                    self.base + position,
                    "an end-of-contents marker with zero length",
                ));
            }
            match length {
                BerLength::Definite(n) => {
                    let end = after_length.checked_add(n).ok_or_else(|| {
                        DecodeError::new(self.base + position, "a length that does not overflow")
                    })?;
                    if end > self.bytes.len() {
                        return Err(DecodeError::new(
                            self.base + position,
                            "contents that fit within the input",
                        ));
                    }
                    position = end;
                }
                BerLength::Indefinite => {
                    if !tag.is_constructed() {
                        return Err(DecodeError::new(
                            self.base + position,
                            "a definite length; only constructed elements may be indefinite",
                        ));
                    }
                    open += 1;
                    if open > Self::MAX_INDEFINITE_DEPTH {
                        return Err(DecodeError::new(
                            self.base + position,
                            "indefinite lengths nested within the depth limit",
                        ));
                    }
                    position = after_length;
                }
            }
        }
    }
}

impl<'a> Iterator for BerReader<'a> {
    type Item = Result<BerElement<'a>, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A constructed context-specific 0 holding two integers.
    const NESTED: [u8; 8] = [0xA0, 0x06, 0x02, 0x01, 0x2A, 0x02, 0x01, 0x07];

    fn one(bytes: &[u8]) -> BerElement<'_> {
        BerReader::new(bytes).next().unwrap().unwrap()
    }

    #[test]
    fn a_constructed_element_yields_its_children() {
        let outer = one(&NESTED);
        assert_eq!(outer.tag().class(), TagClass::ContextSpecific);
        assert_eq!(outer.tag().number(), 0);
        assert!(outer.tag().is_constructed());
        assert_eq!(outer.encoded_len(), 8);

        let children: Vec<_> = outer
            .children()
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].as_u64().unwrap(), 42);
        assert_eq!(children[1].as_u64().unwrap(), 7);
        // Offsets are reported against the original input.
        assert_eq!(children[0].offset(), 2);
        assert_eq!(children[1].offset(), 5);
    }

    /// Contents are borrowed, so walking a large file copies nothing.
    #[test]
    fn contents_borrow_from_the_input() {
        let outer = one(&NESTED);
        let content = outer.content();
        assert_eq!(content.as_ptr(), NESTED[2..].as_ptr());
    }

    #[test]
    fn a_primitive_element_has_no_children() {
        let primitive = one(&[0x02, 0x01, 0x2A]);
        assert!(!primitive.tag().is_constructed());
        assert!(primitive.children().is_err());
        assert_eq!(primitive.as_u64().unwrap(), 42);
    }

    // ── Minimality ───────────────────────────────────────────────────────────

    /// BER leaves the length form to the sender (X.690 §8.1.3.5): a long
    /// form that would fit the short form, or one with leading zero octets,
    /// is a valid encoding of the same length.
    #[test]
    fn a_non_minimal_length_is_accepted() {
        let element = one(&[0x04, 0x81, 0x05, 1, 2, 3, 4, 5]);
        assert_eq!(element.content(), &[1, 2, 3, 4, 5]);
        let element = one(&[0x04, 0x82, 0x00, 0x02, 9, 9]);
        assert_eq!(element.content(), &[9, 9]);
        // The reserved form 0xFF stays refused.
        assert!(BerReader::new(&[0x04, 0xFF]).next().unwrap().is_err());
        // A genuine long form is fine too.
        let long = [&[0x04u8, 0x81, 0x80][..], &[0xAA; 128][..]].concat();
        let element = one(&long);
        assert_eq!(element.content().len(), 128);
    }

    #[test]
    fn a_non_minimal_tag_number_is_rejected() {
        // High-tag form with a leading 0x80 padding octet.
        assert!(
            BerReader::new(&[0x1F, 0x80, 0x7F, 0x00])
                .next()
                .unwrap()
                .is_err()
        );
        // High-tag form encoding a number the short form covers.
        assert!(BerReader::new(&[0x1F, 0x05, 0x00]).next().unwrap().is_err());
        // …while a genuine high tag decodes.
        let element = one(&[0x1F, 0x81, 0x00, 0x00]);
        assert_eq!(element.tag().number(), 128);
    }

    /// X.690 §8.3 requires the shortest integer form, so a padded one is a
    /// second encoding of one value.
    #[test]
    fn a_non_minimal_integer_is_rejected() {
        assert!(one(&[0x02, 0x02, 0x00, 0x2A]).as_u64().is_err());
        // A leading zero *is* required when the next octet's top bit is set,
        // because otherwise the value would read as negative.
        assert_eq!(one(&[0x02, 0x02, 0x00, 0x80]).as_u64().unwrap(), 128);
        // A negative integer is not a length, a count or an identifier.
        assert!(one(&[0x02, 0x01, 0xFF]).as_u64().is_err());
        assert!(one(&[0x02, 0x00]).as_u64().is_err(), "empty");
    }

    // ── Bounds ───────────────────────────────────────────────────────────────

    /// A truncated record must be reported, never partially decoded.
    #[test]
    fn contents_running_past_the_input_are_rejected() {
        assert!(
            BerReader::new(&[0x04, 0x0A, 0x01, 0x02])
                .next()
                .unwrap()
                .is_err()
        );
        assert!(BerReader::new(&[0x04]).next().unwrap().is_err());
        assert!(BerReader::new(&[0x02, 0x81]).next().unwrap().is_err());
    }

    #[test]
    fn an_absurd_length_is_rejected_rather_than_allocated() {
        // Nine length octets exceeds what X.690 needs for any real record.
        assert!(
            BerReader::new(&[0x04, 0x89, 1, 1, 1, 1, 1, 1, 1, 1, 1])
                .next()
                .unwrap()
                .is_err()
        );
        // 0xFF is the reserved length form.
        assert!(BerReader::new(&[0x04, 0xFF]).next().unwrap().is_err());
    }

    /// A malformed element stops the walk: a decoder that skips what it could
    /// not read produces a record missing exactly the fields it failed on.
    #[test]
    fn a_malformed_element_stops_the_walk() {
        let bytes = [0x02, 0x01, 0x2A, 0x04, 0x7F, 0x00];
        let results: Vec<_> = BerReader::new(&bytes).collect();
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
    }

    // ── Indefinite length ────────────────────────────────────────────────────

    #[test]
    fn an_indefinite_length_element_ends_at_its_marker() {
        // Constructed, indefinite, holding one INTEGER, then 00 00.
        let bytes = [0xA0, 0x80, 0x02, 0x01, 0x2A, 0x00, 0x00];
        let element = one(&bytes);

        assert_eq!(element.length(), BerLength::Indefinite);
        assert_eq!(element.content(), &[0x02, 0x01, 0x2A]);
        assert_eq!(element.encoded_len(), bytes.len());

        let children: Vec<_> = element
            .children()
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].as_u64().unwrap(), 42);
    }

    /// Two zero octets occur inside contents all the time, so the scan walks
    /// elements rather than searching for the byte pair.
    #[test]
    fn a_zero_pair_inside_contents_is_not_mistaken_for_the_marker() {
        // An OCTET STRING whose contents are 00 00, then the real marker.
        let bytes = [0xA0, 0x80, 0x04, 0x02, 0x00, 0x00, 0x00, 0x00];
        let element = one(&bytes);
        assert_eq!(element.content(), &[0x04, 0x02, 0x00, 0x00]);
        assert_eq!(element.encoded_len(), bytes.len());
    }

    #[test]
    fn an_indefinite_length_primitive_is_rejected() {
        // Only constructed elements may use the indefinite form.
        assert!(
            BerReader::new(&[0x04, 0x80, 0x00, 0x00])
                .next()
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn a_missing_end_of_contents_marker_is_rejected() {
        assert!(
            BerReader::new(&[0xA0, 0x80, 0x02, 0x01, 0x2A])
                .next()
                .unwrap()
                .is_err()
        );
    }

    // ── Depth ────────────────────────────────────────────────────────────────

    /// Nesting is attacker-controlled, so the bound is mandatory and exceeding
    /// it is reported rather than crashing.
    #[test]
    fn deep_nesting_is_reported_rather_than_exhausting_the_stack() {
        // 5 000 nested constructed elements, each wrapping the next.
        let mut bytes = vec![0x05u8, 0x00]; // a NULL at the core
        for _ in 0..5_000 {
            let len = bytes.len();
            assert!(len < 128, "kept short so the length stays one octet");
            let mut wrapped = vec![0xA0, u8::try_from(len).unwrap()];
            wrapped.extend_from_slice(&bytes);
            bytes = wrapped;
            if bytes.len() >= 120 {
                break;
            }
        }

        let element = one(&bytes);
        assert!(element.validate(1_000).is_ok());
        assert!(
            element.validate(2).is_err(),
            "a shallow bound must refuse to descend"
        );
    }

    #[test]
    fn validate_counts_every_element_it_visits() {
        let outer = one(&NESTED);
        assert_eq!(
            outer.validate(10).unwrap(),
            3,
            "the outer plus two children"
        );

        let primitive = one(&[0x02, 0x01, 0x2A]);
        assert_eq!(primitive.validate(10).unwrap(), 1);
    }

    #[test]
    fn validate_reports_a_malformed_child() {
        // Outer claims 4 octets; the inner element claims more than it has.
        let bytes = [0xA0, 0x04, 0x04, 0x7F, 0x00, 0x00];
        assert!(one(&bytes).validate(10).is_err());
    }

    // ── Searching ────────────────────────────────────────────────────────────

    #[test]
    fn find_searches_one_level_only() {
        // Outer holds a nested constructed element, which holds tag 1.
        let bytes = [0xA0, 0x05, 0xA1, 0x03, 0x02, 0x01, 0x2A];
        let outer = one(&bytes);

        let nested_tag = BerTag::new(TagClass::ContextSpecific, true, 1).unwrap();
        assert!(outer.find(nested_tag).unwrap().is_some());

        // The INTEGER lives one level deeper and is deliberately not found.
        let integer_tag = BerTag::new(TagClass::Universal, false, 2).unwrap();
        assert!(outer.find(integer_tag).unwrap().is_none());
        assert!(
            outer
                .find(nested_tag)
                .unwrap()
                .unwrap()
                .find(integer_tag)
                .unwrap()
                .is_some()
        );
    }

    // ── Tag encoding ─────────────────────────────────────────────────────────

    #[test]
    fn tag_classes_round_trip_through_their_bits() {
        for class in TagClass::ALL {
            assert_eq!(TagClass::from_bits(class.to_bits()), *class);
        }
        assert_eq!(TagClass::from_bits(0xA0), TagClass::ContextSpecific);
        assert_eq!(TagClass::from_bits(0x02), TagClass::Universal);
    }

    #[test]
    fn tags_render_legibly() {
        let tag = BerTag::new(TagClass::ContextSpecific, true, 3).unwrap();
        assert_eq!(tag.to_string(), "CONTEXT 3 (constructed)");
        let primitive = BerTag::new(TagClass::Universal, false, 2).unwrap();
        assert_eq!(primitive.to_string(), "UNIVERSAL 2");
        assert_eq!(BerTag::new(TagClass::Universal, false, u32::MAX), None);
    }

    #[test]
    fn an_end_of_contents_tag_is_recognised() {
        let eoc = BerTag::new(TagClass::Universal, false, 0).unwrap();
        assert!(eoc.is_end_of_contents());
        let not_eoc = BerTag::new(TagClass::ContextSpecific, false, 0).unwrap();
        assert!(!not_eoc.is_end_of_contents());
    }

    #[test]
    fn an_empty_input_yields_nothing() {
        assert_eq!(BerReader::new(&[]).count(), 0);
        assert_eq!(BerReader::new(&[]).remaining(), 0);
    }
}
