#![doc = include_str!("lib.md")]

pub mod gsm7;
mod normalise;
mod segment;
mod translit;

pub use gsm7::{is_gsm7, is_gsm7_str};
pub use normalise::normalise;
pub use segment::{GSM7_CONCATENATED, GSM7_SINGLE, UCS2_CONCATENATED, UCS2_SINGLE};
pub use translit::{replacement_for, transliterate_to_gsm7, Replacement};

use segment::pack;

/// The two encodings an SMS body can be carried in.
///
/// Maps onto the `Encoding` enum in `schema.cstack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmsEncoding {
    /// GSM 03.38 default alphabet, 7 bits per character.
    Gsm7,
    /// UTF-16BE, 16 bits per code unit.
    Ucs2,
}

impl SmsEncoding {
    /// The lowercase name used by the `Encoding` enum in the schema.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SmsEncoding::Gsm7 => "gsm7",
            SmsEncoding::Ucs2 => "ucs2",
        }
    }
}

impl std::fmt::Display for SmsEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A character that forced the body out of GSM-7.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffendingChar {
    /// The character itself.
    pub ch: char,
    /// Its byte offset in the analysed body.
    pub offset: usize,
    /// A GSM-7 replacement, if one exists. `None` for anything with no sensible
    /// Latin equivalent — CJK, emoji, `°`.
    pub replacement: Option<String>,
}

/// What [`analyse`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodingReport {
    /// The encoding the body requires.
    pub encoding: SmsEncoding,
    /// Length in encoding units — septets or UTF-16 code units. GSM-7 escape
    /// pairs and UTF-16 surrogate pairs each count as 2.
    pub length: usize,
    /// Parts the body will be split into. Saturates at 255, the UDH ceiling.
    pub segments: u8,
    /// Units available per part: 160/153 for GSM-7, 70/67 for UCS-2, depending
    /// on whether the message is concatenated.
    pub per_segment: usize,
    /// Every character that forced UCS-2, in input order, with duplicates — the
    /// admin composer highlights each occurrence.
    pub offending: Vec<OffendingChar>,
    /// A transliterated body that would fit GSM-7. `Some` only when the body is
    /// UCS-2 *and* transliteration would actually rescue it.
    pub suggestion: Option<String>,
    /// How many characters cost two septets because they are in the GSM-7
    /// extension table. Always 0 under UCS-2.
    pub escapes: usize,
}

impl EncodingReport {
    /// Units left before the current segment count increases.
    #[must_use]
    pub fn remaining_in_segment(&self) -> usize {
        (self.per_segment * self.segments as usize).saturating_sub(self.length)
    }
}

/// Analyse a message body.
///
/// Does **not** normalise: [`normalise`] runs unconditionally on the send path
/// before this, so analysing a raw body is a preview of what the caller
/// actually submitted. `previewMessage` calls `analyse(&normalise(body))`.
///
/// `suggestion` is computed from `body` alone, so it is still useful on a body
/// that has not been normalised.
#[must_use]
pub fn analyse(body: &str) -> EncodingReport {
    let mut offending = Vec::new();
    let mut escapes = 0usize;
    let mut costs = Vec::with_capacity(body.chars().count());

    for (offset, ch) in body.char_indices() {
        match gsm7::classify(ch) {
            Some(g) => {
                if g.septets() == 2 {
                    escapes += 1;
                }
                costs.push(g.septets());
            }
            None => offending.push(OffendingChar {
                ch,
                offset,
                replacement: replacement_for(ch),
            }),
        }
    }

    if offending.is_empty() {
        let p = pack(&costs, GSM7_SINGLE, GSM7_CONCATENATED);
        return EncodingReport {
            encoding: SmsEncoding::Gsm7,
            length: p.length,
            segments: p.segments,
            per_segment: p.per_segment,
            offending,
            suggestion: None,
            escapes,
        };
    }

    let costs: Vec<usize> = body.chars().map(char::len_utf16).collect();
    let p = pack(&costs, UCS2_SINGLE, UCS2_CONCATENATED);

    // Only offer a rewrite that actually changes the encoding. A body that is
    // still UCS-2 after transliteration has been mangled for nothing.
    let (candidate, _) = transliterate_to_gsm7(body);
    let suggestion = (is_gsm7_str(&candidate) && candidate != body).then_some(candidate);

    EncodingReport {
        encoding: SmsEncoding::Ucs2,
        length: p.length,
        segments: p.segments,
        per_segment: p.per_segment,
        offending,
        suggestion,
        escapes: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ascii_is_one_gsm7_segment() {
        let r = analyse("Votre code est 4821");
        assert_eq!(r.encoding, SmsEncoding::Gsm7);
        assert_eq!(r.length, 19);
        assert_eq!(r.segments, 1);
        assert_eq!(r.per_segment, GSM7_SINGLE);
        assert!(r.offending.is_empty());
        assert_eq!(r.suggestion, None);
    }

    #[test]
    fn french_accents_do_not_force_ucs2() {
        let r = analyse("Vous avez déjà été crédité de 5000 FCFA. Où ? à l'agence.");
        assert_eq!(r.encoding, SmsEncoding::Gsm7);
        assert_eq!(r.segments, 1);
    }

    #[test]
    fn but_the_uppercase_forms_of_those_same_accents_mostly_are_not() {
        // Only É is in the alphabet. À È Ù Ì Ò are not — so a body that is fine
        // in sentence case breaks the moment someone shouts it.
        assert_eq!(analyse("ÉTÉ").encoding, SmsEncoding::Gsm7);
        for body in ["À BIENTÔT", "OÙ ?", "DÉJÀ"] {
            assert_eq!(analyse(body).encoding, SmsEncoding::Ucs2, "{body}");
        }
    }

    #[test]
    fn c_cedilla_forces_ucs2_and_halves_the_body() {
        let r = analyse("Votre code a été reçu");
        assert_eq!(r.encoding, SmsEncoding::Ucs2);
        assert_eq!(r.per_segment, UCS2_SINGLE);
        assert_eq!(r.offending.len(), 1);
        assert_eq!(r.offending[0].ch, 'ç');
        assert_eq!(r.offending[0].replacement.as_deref(), Some("c"));
        assert_eq!(r.suggestion.as_deref(), Some("Votre code a été recu"));
    }

    #[test]
    fn typographic_apostrophe_forces_ucs2_on_its_own() {
        let r = analyse("l\u{2019}agence");
        assert_eq!(r.encoding, SmsEncoding::Ucs2);
        assert_eq!(r.offending[0].ch, '\u{2019}');
        assert_eq!(r.suggestion.as_deref(), Some("l'agence"));
        // And normalising first removes the problem entirely.
        assert_eq!(
            analyse(&normalise("l\u{2019}agence")).encoding,
            SmsEncoding::Gsm7
        );
    }

    #[test]
    fn euro_sign_costs_two_septets() {
        let r = analyse("5€");
        assert_eq!(r.encoding, SmsEncoding::Gsm7);
        assert_eq!(r.length, 3);
        assert_eq!(r.escapes, 1);
    }

    #[test]
    fn offending_offsets_are_byte_offsets_into_the_input() {
        let body = "été reçu";
        let r = analyse(body);
        let off = r.offending[0].offset;
        assert_eq!(body[off..].chars().next(), Some('ç'));
    }

    #[test]
    fn no_suggestion_when_transliteration_cannot_rescue_the_body() {
        let r = analyse("Votre reçu 好");
        assert_eq!(r.encoding, SmsEncoding::Ucs2);
        assert_eq!(r.suggestion, None);
        assert_eq!(r.offending.len(), 2);
        assert_eq!(r.offending[1].replacement, None);
    }

    #[test]
    fn emoji_count_two_ucs2_units() {
        let r = analyse("ok 🙂");
        assert_eq!(r.encoding, SmsEncoding::Ucs2);
        assert_eq!(r.length, 5); // o k space + surrogate pair
    }

    #[test]
    fn remaining_in_segment_counts_down_to_the_next_part() {
        let r = analyse(&"a".repeat(150));
        assert_eq!(r.remaining_in_segment(), 10);
    }

    #[test]
    fn empty_body_is_one_empty_gsm7_segment() {
        let r = analyse("");
        assert_eq!(r.encoding, SmsEncoding::Gsm7);
        assert_eq!((r.length, r.segments), (0, 1));
    }
}
