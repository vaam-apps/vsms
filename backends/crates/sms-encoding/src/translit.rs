#![doc = include_str!("translit.md")]

use unicode_normalization::UnicodeNormalization;

use crate::gsm7::{is_gsm7, is_gsm7_str};
use crate::normalise::substitute;

/// Characters with no useful Unicode decomposition, mapped by hand.
const EXPLICIT: &[(char, &str)] = &[
    ('œ', "oe"),
    ('Œ', "OE"),
    ('\u{00ab}', "\""), // « left-pointing double angle quotation mark
    ('\u{00bb}', "\""), // » right-pointing double angle quotation mark
    ('\u{2039}', "'"),  // ‹ single left-pointing angle quotation mark
    ('\u{203a}', "'"),  // › single right-pointing angle quotation mark
    ('\u{2022}', "-"),  // • bullet
    ('\u{00b7}', "."),  // · middle dot
    ('\u{2122}', "(TM)"),
    ('\u{00a9}', "(C)"),
    ('\u{00ae}', "(R)"),
    ('\u{2116}', "No"), // № numero sign
];

/// One substitution made by [`transliterate_to_gsm7`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    /// The character that was replaced.
    pub from: char,
    /// What it was replaced with. May be empty, or longer than one character.
    pub to: String,
    /// Byte offset of `from` in the *input* string.
    pub offset: usize,
}

/// Suggest a GSM-7 replacement for a single character.
///
/// `None` means the character is either already encodable or has no sensible
/// GSM-7 equivalent.
#[must_use]
pub fn replacement_for(c: char) -> Option<String> {
    if is_gsm7(c) {
        return None;
    }
    if let Some(to) = substitute(c) {
        return Some(to.to_string());
    }
    if let Some(&(_, to)) = EXPLICIT.iter().find(|&&(from, _)| from == c) {
        return Some(to.to_string());
    }
    let stripped: String = c.nfd().filter(|&d| !is_combining_mark(d)).collect();
    if !stripped.is_empty() && stripped != c.to_string() && is_gsm7_str(&stripped) {
        return Some(stripped);
    }
    None
}

/// Rewrite `body` so that as much of it as possible fits GSM 03.38.
///
/// Returns the rewritten body and every substitution made, in input order. The
/// result is *not* guaranteed to be GSM-7 encodable — check it with
/// [`crate::gsm7::is_gsm7_str`], or let [`crate::analyse`] decide whether the
/// rewrite is worth offering.
///
/// ```
/// use sms_encoding::transliterate_to_gsm7;
///
/// // `ç` has no GSM-7 representation on its own, but decomposes (NFD) into
/// // `c` plus a combining cedilla — stripping the combining mark rescues it.
/// let (out, replacements) = transliterate_to_gsm7("reçu");
/// assert_eq!(out, "recu");
/// assert_eq!(replacements.len(), 1);
/// assert_eq!(replacements[0].from, 'ç');
/// assert_eq!(replacements[0].to, "c");
///
/// // A character with no sensible GSM-7 equivalent (here, 好) is left
/// // untouched — this function never invents a lossy replacement.
/// let (out, replacements) = transliterate_to_gsm7("好");
/// assert_eq!(out, "好");
/// assert!(replacements.is_empty());
/// ```
#[must_use]
pub fn transliterate_to_gsm7(body: &str) -> (String, Vec<Replacement>) {
    let mut out = String::with_capacity(body.len());
    let mut replacements = Vec::new();
    for (offset, c) in body.char_indices() {
        match replacement_for(c) {
            Some(to) => {
                out.push_str(&to);
                replacements.push(Replacement {
                    from: c,
                    to,
                    offset,
                });
            }
            None => out.push(c),
        }
    }
    (out, replacements)
}

/// Combining diacritical marks, which is all NFD produces for Latin script.
///
/// Deliberately narrow: this crate strips accents off Latin letters, it does
/// not attempt to romanise other scripts.
fn is_combining_mark(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036f}'   // combining diacritical marks
        | '\u{1ab0}'..='\u{1aff}' // combining diacritical marks extended
        | '\u{1dc0}'..='\u{1dff}' // combining diacritical marks supplement
        | '\u{20d0}'..='\u{20ff}' // combining diacritical marks for symbols
        | '\u{fe20}'..='\u{fe2f}' // combining half marks
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cedilla_is_stripped_by_decomposition() {
        let (out, reps) = transliterate_to_gsm7("reçu");
        assert_eq!(out, "recu");
        assert_eq!(reps.len(), 1);
        assert_eq!(reps[0].from, 'ç');
        assert_eq!(reps[0].to, "c");
        assert_eq!(reps[0].offset, 2); // 'r','e' are one byte each
    }

    #[test]
    fn circumflexes_and_diaereses_are_stripped() {
        let (out, _) = transliterate_to_gsm7("forêt Noël aiguë ÿ");
        assert_eq!(out, "foret Noel aigue y");
        assert!(is_gsm7_str(&out));
    }

    #[test]
    fn uppercase_accents_absent_from_gsm7_are_stripped_but_present_ones_are_kept() {
        // É and Ä are in the alphabet; À, È, Ù, Ê are not.
        let (out, _) = transliterate_to_gsm7("ÉÄ ÀÈÙÊ");
        assert_eq!(out, "ÉÄ AEUE");
    }

    #[test]
    fn ligatures_and_guillemets_use_the_explicit_table() {
        let (out, _) = transliterate_to_gsm7("cœur «test» •");
        assert_eq!(out, "coeur \"test\" -");
    }

    #[test]
    fn normalisation_substitutions_are_included() {
        let (out, _) = transliterate_to_gsm7("l\u{2019}appli\u{2026}");
        assert_eq!(out, "l'appli...");
    }

    #[test]
    fn already_encodable_characters_are_never_touched() {
        let body = "Votre code est 4821 (été, à jour) @ 12h30 - 50%";
        let (out, reps) = transliterate_to_gsm7(body);
        assert_eq!(out, body);
        assert!(reps.is_empty());
    }

    #[test]
    fn untransliterable_characters_are_left_alone() {
        let (out, reps) = transliterate_to_gsm7("20°C 好 🙂");
        assert_eq!(out, "20°C 好 🙂");
        assert!(reps.is_empty());
        assert!(!is_gsm7_str(&out));
    }

    #[test]
    fn every_explicit_entry_is_needed_and_lands_inside_gsm7() {
        for &(from, to) in EXPLICIT {
            assert!(!is_gsm7(from), "{from:?} is already encodable");
            assert!(is_gsm7_str(to), "{from:?} -> {to:?} is not encodable");
        }
    }
}
