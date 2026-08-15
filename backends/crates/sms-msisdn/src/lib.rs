#![doc = include_str!("lib.md")]

mod operator;
mod plan;

pub use operator::OperatorPrefixTable;
pub use plan::{COUNTRY_CODE, LineType, classify};

/// Why a string is not a usable Cameroon MSISDN.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MsisdnError {
    /// Nothing but separators.
    #[error("empty number")]
    Empty,

    /// A character that is neither a digit nor a recognised separator.
    #[error("unexpected character {ch:?} in number")]
    InvalidCharacter {
        /// The offending character.
        ch: char,
    },

    /// An explicit international prefix for some country other than Cameroon.
    #[error("country code +{cc} is not supported, only +{COUNTRY_CODE}")]
    UnsupportedCountry {
        /// The digits that followed the `+` or `00`.
        cc: String,
    },

    /// Eight digits: a pre-November-2014 number. Prefixing `6` or `2` would be
    /// a guess about which, so the caller is told to fix its data instead.
    #[error(
        "{digits} is an eight-digit pre-2014 number; it needs a leading 6 (mobile) or 2 (fixed)"
    )]
    LegacyEightDigit {
        /// The eight digits as given.
        digits: String,
    },

    /// Not 9 digits (nor the 8-digit `88x` short form).
    #[error("expected 9 national digits, got {got}")]
    BadLength {
        /// How many national digits were found.
        got: usize,
    },

    /// Right length, but in no assigned block — `63x`, `643`–`649`, or a
    /// leading digit the plan does not use.
    #[error("{national} is not in an assigned range")]
    Unallocated {
        /// The national significant number.
        national: String,
    },

    /// A valid number, but not one an SMS can reach. Only from
    /// [`Msisdn::parse_mobile`].
    #[error("{national} is a {line_type} number, not a mobile")]
    NotMobile {
        /// The national significant number.
        national: String,
        /// What it turned out to be.
        line_type: LineType,
    },
}

/// A Cameroon number in canonical E.164 form.
///
/// Constructing one is the only way to get here, so a `Msisdn` is always
/// `+237` plus a national number inside an assigned block. Whether it is
/// *addressable* is a separate question — ask [`Msisdn::line_type`], or build
/// it with [`Msisdn::parse_mobile`], which refuses everything else.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Msisdn {
    e164: String,
    line_type: LineType,
}

/// Separators a human or a spreadsheet might insert.
const SEPARATORS: &[char] = &[
    ' ', '\t', '-', '.', '/', '(', ')', '[', ']', '\u{00a0}', '\u{202f}',
];

impl Msisdn {
    /// Parse any Cameroon number.
    ///
    /// Accepts `+237…`, `00237…`, `237…` and a bare national number, with
    /// spaces, dots, dashes, slashes, brackets and non-breaking spaces
    /// anywhere. Accepts fixed lines and toll-free numbers — use
    /// [`Msisdn::parse_mobile`] on the send path.
    pub fn parse(input: &str) -> Result<Self, MsisdnError> {
        let national = to_national(input)?;
        match plan::classify(&national) {
            Some(LineType::Unallocated) | None => Err(MsisdnError::Unallocated { national }),
            Some(line_type) => Ok(Self {
                e164: format!("+{COUNTRY_CODE}{national}"),
                line_type,
            }),
        }
    }

    /// Parse a number an SMS can actually be delivered to.
    ///
    /// Everything [`Msisdn::parse`] accepts, minus fixed lines and toll-free
    /// numbers. This is what the API boundary calls.
    pub fn parse_mobile(input: &str) -> Result<Self, MsisdnError> {
        let m = Self::parse(input)?;
        if m.line_type.is_addressable() {
            Ok(m)
        } else {
            Err(MsisdnError::NotMobile {
                national: m.national().to_string(),
                line_type: m.line_type,
            })
        }
    }

    /// The canonical E.164 form, `+237` included.
    #[must_use]
    pub fn as_e164(&self) -> &str {
        &self.e164
    }

    /// The national significant number, without the country code.
    #[must_use]
    pub fn national(&self) -> &str {
        &self.e164[1 + COUNTRY_CODE.len()..]
    }

    /// What kind of line this addresses.
    #[must_use]
    pub const fn line_type(&self) -> LineType {
        self.line_type
    }

    /// Whether an SMS can be delivered here.
    #[must_use]
    pub const fn is_addressable(&self) -> bool {
        self.line_type.is_addressable()
    }

    /// A redacted form for apps with `maskRecipient` set, and for anywhere a
    /// number would otherwise land in a log.
    ///
    /// Keeps the country code and the last two digits: `+237*******56`.
    #[must_use]
    pub fn masked(&self) -> String {
        let national = self.national();
        let keep = 2.min(national.len());
        let hidden = national.len() - keep;
        format!(
            "+{}{}{}",
            COUNTRY_CODE,
            "*".repeat(hidden),
            &national[hidden..]
        )
    }

    /// Consume the number, returning the owned E.164 string.
    #[must_use]
    pub fn into_e164(self) -> String {
        self.e164
    }
}

impl std::fmt::Display for Msisdn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.e164)
    }
}

impl std::str::FromStr for Msisdn {
    type Err = MsisdnError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Strip separators and the country code, leaving national digits.
fn to_national(input: &str) -> Result<String, MsisdnError> {
    let mut digits = String::with_capacity(input.len());
    let mut plus = false;
    for c in input.chars() {
        if SEPARATORS.contains(&c) {
            continue;
        }
        if c == '+' {
            // A `+` is only meaningful once, and only before any digit.
            if plus || !digits.is_empty() {
                return Err(MsisdnError::InvalidCharacter { ch: c });
            }
            plus = true;
            continue;
        }
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            return Err(MsisdnError::InvalidCharacter { ch: c });
        }
    }

    if digits.is_empty() {
        return Err(MsisdnError::Empty);
    }

    // `00` is the ITU international prefix; after it a country code is
    // mandatory, so an unknown one is an error rather than a fall-through.
    let rest = if let Some(rest) = digits.strip_prefix("00") {
        match rest.strip_prefix(COUNTRY_CODE) {
            Some(national) => national.to_string(),
            None => {
                return Err(MsisdnError::UnsupportedCountry {
                    cc: rest.chars().take(3).collect(),
                });
            }
        }
    } else if plus {
        match digits.strip_prefix(COUNTRY_CODE) {
            Some(national) => national.to_string(),
            None => {
                return Err(MsisdnError::UnsupportedCountry {
                    cc: digits.chars().take(3).collect(),
                });
            }
        }
    } else if digits.len() > 9 {
        // No explicit international marker, but too long to be national. The
        // only reading that can be right is a bare country code.
        match digits.strip_prefix(COUNTRY_CODE) {
            Some(national) => national.to_string(),
            None => return Err(MsisdnError::BadLength { got: digits.len() }),
        }
    } else {
        digits
    };

    match rest.len() {
        // The `88x` short form is the one legitimate 8-digit number.
        8 if rest.starts_with("88") => Ok(rest),
        8 => Err(MsisdnError::LegacyEightDigit { digits: rest }),
        9 => Ok(rest),
        got => Err(MsisdnError::BadLength { got }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_input_form_reaches_the_same_number() {
        for input in [
            "677123456",
            "237677123456",
            "+237677123456",
            "00237677123456",
            "+237 677 12 34 56",
            "(+237) 6-77-12-34-56",
            "\u{00a0}+237\u{202f}677.12.34.56\u{00a0}",
        ] {
            let m = Msisdn::parse(input).unwrap_or_else(|e| panic!("{input:?}: {e}"));
            assert_eq!(m.as_e164(), "+237677123456", "{input:?}");
            assert_eq!(m.national(), "677123456");
        }
    }

    #[test]
    fn other_country_codes_are_refused_rather_than_mangled() {
        assert_eq!(
            Msisdn::parse("+33612345678"),
            Err(MsisdnError::UnsupportedCountry { cc: "336".into() })
        );
        assert_eq!(
            Msisdn::parse("0033612345678"),
            Err(MsisdnError::UnsupportedCountry { cc: "336".into() })
        );
    }

    #[test]
    fn eight_digit_legacy_numbers_get_their_own_error() {
        // Prefixing 6 vs 2 is a guess, and the wrong guess sends an OTP to a
        // stranger. Make the caller fix its data.
        assert_eq!(
            Msisdn::parse("77123456"),
            Err(MsisdnError::LegacyEightDigit {
                digits: "77123456".into()
            })
        );
    }

    #[test]
    fn the_toll_free_short_form_is_not_mistaken_for_a_legacy_number() {
        let m = Msisdn::parse("88123456").unwrap();
        assert_eq!(m.line_type(), LineType::TollFree);
        assert_eq!(m.as_e164(), "+23788123456");
    }

    #[test]
    fn unassigned_ranges_are_rejected() {
        assert_eq!(
            Msisdn::parse("637123456"),
            Err(MsisdnError::Unallocated {
                national: "637123456".into()
            })
        );
        assert!(matches!(
            Msisdn::parse("645123456"),
            Err(MsisdnError::Unallocated { .. })
        ));
        assert!(Msisdn::parse("642123456").is_ok());
    }

    #[test]
    fn fixed_lines_parse_but_do_not_pass_the_send_path() {
        let m = Msisdn::parse("+237222123456").unwrap();
        assert_eq!(m.line_type(), LineType::FixedLine);
        assert!(!m.is_addressable());
        assert_eq!(
            Msisdn::parse_mobile("+237222123456"),
            Err(MsisdnError::NotMobile {
                national: "222123456".into(),
                line_type: LineType::FixedLine
            })
        );
    }

    #[test]
    fn junk_is_named_precisely() {
        assert_eq!(Msisdn::parse(""), Err(MsisdnError::Empty));
        assert_eq!(Msisdn::parse("   - "), Err(MsisdnError::Empty));
        assert_eq!(
            Msisdn::parse("677abc456"),
            Err(MsisdnError::InvalidCharacter { ch: 'a' })
        );
        assert_eq!(
            Msisdn::parse("+237+677123456"),
            Err(MsisdnError::InvalidCharacter { ch: '+' })
        );
        assert_eq!(
            Msisdn::parse("67712345"),
            Err(MsisdnError::LegacyEightDigit {
                digits: "67712345".into()
            })
        );
        assert_eq!(
            Msisdn::parse("6771234567"),
            Err(MsisdnError::BadLength { got: 10 })
        );
    }

    #[test]
    fn e164_length_fits_the_schema_column() {
        // Message.msisdn is @length(min: 12, max: 15).
        for input in ["677123456", "88123456", "222123456"] {
            let len = Msisdn::parse(input).unwrap().as_e164().len();
            assert!(
                (12..=15).contains(&len),
                "{input} produced {len} characters"
            );
        }
    }

    #[test]
    fn masking_keeps_the_country_code_and_two_digits() {
        assert_eq!(
            Msisdn::parse("677123456").unwrap().masked(),
            "+237*******56"
        );
        assert_eq!(Msisdn::parse("88123456").unwrap().masked(), "+237******56");
    }

    #[test]
    fn round_trips_through_display_and_from_str() {
        let m: Msisdn = "677 12 34 56".parse().unwrap();
        assert_eq!(m.to_string(), "+237677123456");
        assert_eq!(m.to_string().parse::<Msisdn>().unwrap(), m);
    }
}
