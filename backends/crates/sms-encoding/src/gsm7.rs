#![doc = include_str!("gsm7.md")]

/// The 128 septet values of the default alphabet, indexed by septet.
///
/// Index 0x1B is ESC and introduces the extension table; it carries no
/// character of its own and is stored here as U+001B so the array stays a
/// straight index-to-character map.
pub(crate) const BASIC: [char; 128] = [
    '@', '£', '$', '¥', 'è', 'é', 'ù', 'ì', // 0x00
    'ò', 'Ç', '\n', 'Ø', 'ø', '\r', 'Å', 'å', // 0x08
    'Δ', '_', 'Φ', 'Γ', 'Λ', 'Ω', 'Π', 'Ψ', // 0x10
    'Σ', 'Θ', 'Ξ', '\u{1b}', 'Æ', 'æ', 'ß', 'É', // 0x18  (0x1B = ESC)
    ' ', '!', '"', '#', '¤', '%', '&', '\'', // 0x20
    '(', ')', '*', '+', ',', '-', '.', '/', // 0x28
    '0', '1', '2', '3', '4', '5', '6', '7', // 0x30
    '8', '9', ':', ';', '<', '=', '>', '?', // 0x38
    '¡', 'A', 'B', 'C', 'D', 'E', 'F', 'G', // 0x40
    'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', // 0x48
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', // 0x50
    'X', 'Y', 'Z', 'Ä', 'Ö', 'Ñ', 'Ü', '§', // 0x58
    '¿', 'a', 'b', 'c', 'd', 'e', 'f', 'g', // 0x60
    'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', // 0x68
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', // 0x70
    'x', 'y', 'z', 'ä', 'ö', 'ñ', 'ü', 'à', // 0x78
];

/// The escape byte that introduces an extension-table character.
pub(crate) const ESC: u8 = 0x1b;

/// The extension table, reached as `ESC` followed by the listed septet.
///
/// Each of these costs **two** septets, not one.
pub(crate) const EXTENDED: [(u8, char); 10] = [
    (0x0a, '\u{c}'), // form feed
    (0x14, '^'),
    (0x28, '{'),
    (0x29, '}'),
    (0x2f, '\\'),
    (0x3c, '['),
    (0x3d, '~'),
    (0x3e, ']'),
    (0x40, '|'),
    (0x65, '€'),
];

/// How a character is represented in GSM 03.38, if it is representable at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gsm7Char {
    /// A default-alphabet character. One septet.
    Basic(u8),
    /// An extension-table character. `ESC` plus the septet: two septets.
    Extended(u8),
}

impl Gsm7Char {
    /// Septets on the wire — 1 for basic, 2 for extended.
    #[must_use]
    pub const fn septets(self) -> usize {
        match self {
            Gsm7Char::Basic(_) => 1,
            Gsm7Char::Extended(_) => 2,
        }
    }
}

/// Look a character up in the GSM 03.38 tables.
///
/// Returns `None` for anything that would force the whole message to UCS-2.
#[must_use]
pub fn classify(c: char) -> Option<Gsm7Char> {
    // ESC is a control code, not a character a caller may send. Reject it here
    // so a literal U+001B in a body cannot be mistaken for septet 0x1B.
    if c == '\u{1b}' {
        return None;
    }
    if let Some(i) = BASIC.iter().position(|&b| b == c) {
        // The scan is 128 comparisons at worst on bodies of at most a few
        // hundred characters. Not worth a lookup table.
        #[allow(clippy::cast_possible_truncation)]
        return Some(Gsm7Char::Basic(i as u8));
    }
    EXTENDED
        .iter()
        .find(|&&(_, e)| e == c)
        .map(|&(code, _)| Gsm7Char::Extended(code))
}

/// Whether a character survives GSM-7 encoding.
#[must_use]
pub fn is_gsm7(c: char) -> bool {
    classify(c).is_some()
}

/// Whether every character in `body` survives GSM-7 encoding.
#[must_use]
pub fn is_gsm7_str(body: &str) -> bool {
    body.chars().all(is_gsm7)
}

/// Encode `body` to septet values, `ESC`-prefixing extension characters.
///
/// Returns `None` if any character is outside GSM 03.38. This is the septet
/// stream, *not* the 7-bit packed octets an SMPP PDU carries — packing belongs
/// to the SMPP provider, which is the only place that needs it.
#[must_use]
pub fn to_septets(body: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(body.len());
    for c in body.chars() {
        match classify(c)? {
            Gsm7Char::Basic(b) => out.push(b),
            Gsm7Char::Extended(b) => {
                out.push(ESC);
                out.push(b);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_no_duplicates_outside_esc() {
        let mut seen: Vec<char> = BASIC.iter().copied().filter(|&c| c != '\u{1b}').collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "the basic alphabet has a duplicate");
    }

    #[test]
    fn french_vowel_accents_are_single_septets() {
        // The claim the whole crate exists to defend: these do NOT force UCS-2.
        for c in [
            'è', 'é', 'ù', 'ì', 'ò', 'à', 'É', 'Ä', 'Ö', 'Ñ', 'Ü', 'ä', 'ö', 'ñ', 'ü',
        ] {
            assert_eq!(
                classify(c).map(Gsm7Char::septets),
                Some(1),
                "{c} should be one septet"
            );
        }
    }

    #[test]
    fn lowercase_c_cedilla_is_not_encodable_but_uppercase_is() {
        assert_eq!(classify('ç'), None);
        assert_eq!(classify('Ç'), Some(Gsm7Char::Basic(0x09)));
    }

    #[test]
    fn extension_characters_cost_two() {
        for c in ['€', '[', ']', '{', '}', '\\', '|', '^', '~'] {
            assert_eq!(
                classify(c).map(Gsm7Char::septets),
                Some(2),
                "{c} should be two septets"
            );
        }
    }

    #[test]
    fn backtick_is_the_one_unencodable_printable_ascii() {
        let unencodable: Vec<char> = (0x20u8..0x7f)
            .map(char::from)
            .filter(|&c| !is_gsm7(c))
            .collect();
        assert_eq!(unencodable, vec!['`']);
    }

    #[test]
    fn ascii_dollar_and_at_map_to_their_gsm_positions_not_their_ascii_ones() {
        assert_eq!(classify('$'), Some(Gsm7Char::Basic(0x02)));
        assert_eq!(classify('@'), Some(Gsm7Char::Basic(0x00)));
        assert_eq!(classify('¤'), Some(Gsm7Char::Basic(0x24)));
        assert_eq!(classify('¡'), Some(Gsm7Char::Basic(0x40)));
    }

    #[test]
    fn septets_escape_extension_characters() {
        assert_eq!(to_septets("a€"), Some(vec![0x61, ESC, 0x65]));
        assert_eq!(to_septets("ç"), None);
    }
}
