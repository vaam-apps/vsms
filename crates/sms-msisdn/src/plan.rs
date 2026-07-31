//! The Cameroon numbering plan.
//!
//! Closed 9-digit plan since November 2014, when every subscriber number gained
//! a leading digit: `6` for mobile, `2` for fixed. Eight-digit numbers still sit
//! in customer databases everywhere and are the single most common bad input.
//!
//! Ranges, from the ITU national numbering plan and `libphonenumber`:
//!
//! ```text
//! mobile      (?:24[23]|6(?:[25-9]\d|4[0-2]))\d{6}
//! fixed line  2(?:22|33)\d{6}
//! general     [26]\d{8}|88\d{6,7}
//! ```
//!
//! Note what is *absent* from the mobile range: `63x` and `643`–`649` are not
//! assigned. `640`–`642` are.

/// Cameroon's E.164 country calling code, without the `+`.
pub const COUNTRY_CODE: &str = "237";

/// What kind of line a number addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineType {
    /// A mobile handset. The only kind an SMS can reach.
    Mobile,
    /// A fixed line. Reject at the API boundary.
    FixedLine,
    /// An `88x` shared-cost or toll-free number.
    TollFree,
    /// Inside the general national range but outside every assigned block —
    /// `63x`, `643`–`649`, and anything else the plan has not allocated.
    Unallocated,
}

impl LineType {
    /// Whether an SMS can be delivered to this kind of number.
    #[must_use]
    pub const fn is_addressable(self) -> bool {
        matches!(self, LineType::Mobile)
    }

    /// A short lowercase name, for logs and error payloads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            LineType::Mobile => "mobile",
            LineType::FixedLine => "fixed_line",
            LineType::TollFree => "toll_free",
            LineType::Unallocated => "unallocated",
        }
    }
}

impl std::fmt::Display for LineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classify a national significant number.
///
/// `digits` must already be ASCII digits only. Returns `None` when the number
/// is outside the national plan entirely — wrong length, or a leading digit the
/// plan does not use.
#[must_use]
pub fn classify(digits: &str) -> Option<LineType> {
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let b = digits.as_bytes();
    match b.len() {
        // 88 + 6 digits: the short form of the shared-cost range.
        8 if b.starts_with(b"88") => Some(LineType::TollFree),
        9 => Some(classify_nine(b)),
        _ => None,
    }
}

fn classify_nine(b: &[u8]) -> LineType {
    match b[0] {
        b'6' => {
            // 62, 65, 66, 67, 68, 69 are mobile in full; 64 only for 640-642.
            match (b[1], b[2]) {
                (b'2' | b'5'..=b'9', _) | (b'4', b'0'..=b'2') => LineType::Mobile,
                _ => LineType::Unallocated, // 63x, 643-649
            }
        }
        b'2' => match (b[1], b[2]) {
            // Camtel mobile sits inside the 2 range, not the 6 range.
            (b'4', b'2' | b'3') => LineType::Mobile,
            (b'2', b'2') | (b'3', b'3') => LineType::FixedLine,
            _ => LineType::Unallocated,
        },
        b'8' if b[1] == b'8' => LineType::TollFree,
        _ => LineType::Unallocated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigned_mobile_prefixes() {
        for p in [
            "620", "650", "660", "670", "680", "690", "699", "640", "641", "642",
        ] {
            let n = format!("{p}123456");
            assert_eq!(classify(&n), Some(LineType::Mobile), "{n} should be mobile");
        }
    }

    #[test]
    fn unassigned_mobile_prefixes_are_not_mobile() {
        for p in ["630", "639", "643", "649"] {
            let n = format!("{p}123456");
            assert_eq!(
                classify(&n),
                Some(LineType::Unallocated),
                "{n} should not be mobile"
            );
        }
    }

    #[test]
    fn camtel_mobile_lives_in_the_two_range() {
        assert_eq!(classify("242123456"), Some(LineType::Mobile));
        assert_eq!(classify("243123456"), Some(LineType::Mobile));
        assert_eq!(classify("241123456"), Some(LineType::Unallocated));
    }

    #[test]
    fn fixed_lines_are_recognised_and_not_addressable() {
        assert_eq!(classify("222123456"), Some(LineType::FixedLine));
        assert_eq!(classify("233123456"), Some(LineType::FixedLine));
        assert!(!LineType::FixedLine.is_addressable());
    }

    #[test]
    fn toll_free_comes_in_both_lengths() {
        assert_eq!(classify("88123456"), Some(LineType::TollFree));
        assert_eq!(classify("881234567"), Some(LineType::TollFree));
    }

    #[test]
    fn wrong_lengths_are_outside_the_plan() {
        assert_eq!(classify("67712345"), None); // pre-2014 eight digits
        assert_eq!(classify("6771234567"), None);
        assert_eq!(classify(""), None);
    }

    #[test]
    fn only_mobile_is_addressable() {
        assert!(LineType::Mobile.is_addressable());
        for t in [
            LineType::FixedLine,
            LineType::TollFree,
            LineType::Unallocated,
        ] {
            assert!(!t.is_addressable());
        }
    }
}
