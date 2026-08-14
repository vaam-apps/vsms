//! Unconditional, lossless-to-the-recipient cleanup.
//!
//! Everything replaced here is a typographic substitute for a character that is
//! already in GSM 03.38, or an invisible character that carries no meaning in an
//! SMS. Nobody reading the message on a handset can tell the difference, and a
//! single `’` arriving from a copy-paste out of Word is enough on its own to
//! push a 160-character body to UCS-2 and halve it to 70.
//!
//! Anything *perceptible* — `ç` → `c`, `«` → `"` — is transliteration, is
//! opt-in per app, and lives in [`crate::translit`].

use unicode_normalization::UnicodeNormalization;

/// Typographic substitutions. Left side is never in GSM 03.38; right side
/// always is.
pub(crate) const SUBSTITUTIONS: &[(char, &str)] = &[
    // Quotes. The apostrophe is the single highest-value entry in this table:
    // French bodies acquire it constantly and it is invisible in review.
    ('\u{2018}', "'"),  // ‘ left single quotation mark
    ('\u{2019}', "'"),  // ’ right single quotation mark
    ('\u{201a}', "'"),  // ‚ single low-9 quotation mark
    ('\u{201b}', "'"),  // ‛ single high-reversed-9 quotation mark
    ('\u{2032}', "'"),  // ′ prime
    ('\u{201c}', "\""), // “ left double quotation mark
    ('\u{201d}', "\""), // ” right double quotation mark
    ('\u{201e}', "\""), // „ double low-9 quotation mark
    ('\u{2033}', "\""), // ″ double prime
    // Dashes.
    ('\u{2010}', "-"), // ‐ hyphen
    ('\u{2011}', "-"), // ‑ non-breaking hyphen
    ('\u{2012}', "-"), // ‒ figure dash
    ('\u{2013}', "-"), // – en dash
    ('\u{2014}', "-"), // — em dash
    ('\u{2015}', "-"), // ― horizontal bar
    ('\u{2212}', "-"), // − minus sign
    // Ellipsis. Three septets instead of one is still a win over doubling the
    // entire body.
    ('\u{2026}', "..."), // …
    // Spaces. All of these render as a space and none are in GSM 03.38.
    ('\u{00a0}', " "), // no-break space
    ('\u{2000}', " "), // en quad
    ('\u{2001}', " "), // em quad
    ('\u{2002}', " "), // en space
    ('\u{2003}', " "), // em space
    ('\u{2004}', " "), // three-per-em space
    ('\u{2005}', " "), // four-per-em space
    ('\u{2006}', " "), // six-per-em space
    ('\u{2007}', " "), // figure space
    ('\u{2008}', " "), // punctuation space
    ('\u{2009}', " "), // thin space
    ('\u{200a}', " "), // hair space
    ('\u{202f}', " "), // narrow no-break space
    ('\u{205f}', " "), // medium mathematical space
    ('\u{3000}', " "), // ideographic space
    // Invisibles, dropped outright.
    ('\u{00ad}', ""), // soft hyphen
    ('\u{200b}', ""), // zero-width space
    ('\u{200c}', ""), // zero-width non-joiner
    ('\u{200d}', ""), // zero-width joiner
    ('\u{feff}', ""), // zero-width no-break space / BOM
];

pub(crate) fn substitute(c: char) -> Option<&'static str> {
    SUBSTITUTIONS
        .iter()
        .find(|&&(from, _)| from == c)
        .map(|&(_, to)| to)
}

/// Normalise a message body.
///
/// Composes to NFC first — so a decomposed `e` + U+0301 becomes the `é` that
/// GSM 03.38 actually has — then applies [`SUBSTITUTIONS`].
///
/// Runs on every outbound message unconditionally. It is not a policy decision.
#[must_use]
pub fn normalise(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for c in body.nfc() {
        match substitute(c) {
            Some(replacement) => out.push_str(replacement),
            None => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gsm7::{is_gsm7, is_gsm7_str};

    #[test]
    fn nfc_composes_decomposed_accents_into_gsm7_characters() {
        let decomposed = "e\u{301}te\u{301}"; // "été" written as e + combining acute
        assert!(!is_gsm7_str(decomposed));
        assert_eq!(normalise(decomposed), "été");
        assert!(is_gsm7_str(&normalise(decomposed)));
    }

    #[test]
    fn typographic_apostrophe_becomes_a_straight_one() {
        assert_eq!(normalise("l\u{2019}appli"), "l'appli");
        assert!(is_gsm7_str(&normalise("l\u{2019}appli")));
    }

    #[test]
    fn substitutions_all_land_inside_gsm7() {
        for &(from, to) in SUBSTITUTIONS {
            assert!(
                !is_gsm7(from),
                "{from:?} is already encodable, it does not belong here"
            );
            assert!(is_gsm7_str(to), "{from:?} -> {to:?} is not encodable");
        }
    }

    #[test]
    fn invisibles_are_dropped() {
        assert_eq!(normalise("6\u{200b}77\u{00ad}12"), "67712");
    }

    #[test]
    fn a_body_that_needs_nothing_is_returned_unchanged() {
        let body = "Votre code est 4821. Il expire dans 5 minutes.";
        assert_eq!(normalise(body), body);
    }

    #[test]
    fn normalise_does_not_touch_perceptible_characters() {
        // These are transliteration's business, not normalisation's.
        assert_eq!(normalise("reçu «test»"), "reçu «test»");
    }
}
