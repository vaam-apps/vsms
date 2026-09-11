#![doc = include_str!("lib.md")]

mod operator;
mod plan;
mod region;

use phonenumber::metadata::DATABASE;

pub use operator::OperatorPrefixTable;
pub use plan::{LineType, classify, classify_in};
pub use region::Region;

/// Cameroon's E.164 country calling code, as a number.
///
/// The only country this crate still special-cases, and only for
/// [`MsisdnError::LegacyEightDigit`].
const CM_COUNTRY_CODE: u16 = 237;

/// ITU-T E.164 caps a number at 15 digits, country code included.
const MAX_E164_DIGITS: usize = 15;

/// Why a string is not a usable MSISDN.
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

    /// Not an ISO 3166-1 alpha-2 region code. From [`Region`]'s own
    /// `FromStr`, never from parsing a number.
    #[error("{code:?} is not an ISO 3166-1 alpha-2 region code")]
    UnknownRegion {
        /// The string that was offered as a region.
        code: String,
    },

    /// An explicit international prefix naming a country code E.164 has
    /// never assigned.
    ///
    /// This is not "we do not serve that country" — every country is
    /// supported. Which countries a given tenant may send *to* is a policy
    /// question that belongs above this crate, not a parsing one.
    #[error("no country uses the calling code in +{digits}")]
    UnknownCountryCode {
        /// The digits that followed the international prefix.
        digits: String,
    },

    /// Eight digits in Cameroon: a pre-November-2014 number. Prefixing `6` or
    /// `2` would be a guess about which, so the caller is told to fix its
    /// data instead.
    ///
    /// Cameroon-specific on purpose. Eight digits is an ordinary, valid
    /// length in plenty of countries (Denmark and Norway, for two), so
    /// anywhere else this hint would be nonsense and the number is simply
    /// invalid.
    #[error(
        "{digits} is an eight-digit pre-2014 Cameroon number; it needs a leading 6 (mobile) or 2 (fixed)"
    )]
    LegacyEightDigit {
        /// The eight digits as given.
        digits: String,
    },

    /// A number of a length no assigned range in that country uses — a
    /// truncated or over-long number, rather than an unallocated one.
    #[error("{got} digits is not a usable length for a number in this country")]
    BadLength {
        /// How many digits were found. The national significant number's
        /// digits once a country could be determined, otherwise every digit
        /// in the input.
        got: usize,
    },

    /// A plausible length, but in no assigned range — Cameroon's `63x` and
    /// `642`–`649`, and the equivalent everywhere else.
    #[error("{national} is not in an assigned range")]
    Unallocated {
        /// The national significant number.
        national: String,
    },

    /// A valid number, but not one an SMS can reach. Only from
    /// [`Msisdn::parse_mobile`] and [`Msisdn::parse_mobile_in`].
    #[error("{national} cannot receive an SMS: line type {line_type}")]
    NotAddressable {
        /// The national significant number.
        national: String,
        /// What it turned out to be.
        line_type: LineType,
    },
}

/// A number in canonical E.164 form.
///
/// Constructing one is the only way to get here, so a `Msisdn` is always `+`,
/// a real country calling code, and a national number inside an assigned
/// range — [`LineType::Unallocated`] is unreachable through
/// [`Msisdn::line_type`]. Whether it is *addressable* is a separate question:
/// ask [`Msisdn::line_type`], or build it with [`Msisdn::parse_mobile`],
/// which refuses everything else.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Msisdn {
    e164: String,
    /// Byte offset of the national number within `e164`, i.e. one for the `+`
    /// plus the country code's own digits. Country codes are ASCII digits, so
    /// byte and character offsets coincide.
    national_offset: usize,
    region: Option<Region>,
    line_type: LineType,
}

/// Separators a human or a spreadsheet might insert.
const SEPARATORS: &[char] = &[
    ' ', '\t', '-', '.', '/', '(', ')', '[', ']', '\u{00a0}', '\u{202f}',
];

impl Msisdn {
    /// Parse any number, reading a bare national number as Cameroonian.
    ///
    /// Accepts `+…`, `00…` (Cameroon's own international prefix) and a bare
    /// national number, with spaces, dots, dashes, slashes, brackets and
    /// non-breaking spaces anywhere. Accepts fixed lines and toll-free
    /// numbers — use [`Msisdn::parse_mobile`] on the send path.
    ///
    /// ```
    /// use sms_msisdn::Msisdn;
    ///
    /// // Cameroon needs no configuration to stay the default.
    /// assert_eq!(Msisdn::parse("677123456").unwrap().as_e164(), "+237677123456");
    /// // Anything with its own country code is read as that country's.
    /// assert_eq!(Msisdn::parse("00447912345678").unwrap().as_e164(), "+447912345678");
    /// ```
    pub fn parse(input: &str) -> Result<Self, MsisdnError> {
        Self::parse_in(input, Region::CM)
    }

    /// Parse any number, reading a bare national number against `region`.
    ///
    /// `region` is consulted only when the input carries no country code of
    /// its own; a leading `+` names the country and wins.
    ///
    /// **An international prefix that is not `+` is the *dialling region's*
    /// own, not a universal `00`.** Cameroon, most of Europe and much of
    /// Africa use `00`; North America uses `011`; Nigeria uses `009`. So
    /// `parse_in("0033612345678", Region::CM)` is the French number, while
    /// the same string against `"US"` is not — from an American handset the
    /// caller would have dialled `011 33 …`. Checked against the metadata,
    /// not assumed. A `+` is unambiguous everywhere and is what an
    /// integrator should send.
    ///
    /// ```
    /// use sms_msisdn::{Msisdn, Region};
    ///
    /// let de: Region = "DE".parse().unwrap();
    /// assert_eq!(
    ///     Msisdn::parse_in("0151 12345678", de).unwrap().as_e164(),
    ///     "+4915112345678",
    /// );
    ///
    /// // The region does not override a country code the caller supplied.
    /// assert_eq!(Msisdn::parse_in("+237677123456", de).unwrap().as_e164(), "+237677123456");
    /// ```
    pub fn parse_in(input: &str, region: Region) -> Result<Self, MsisdnError> {
        let candidate = sanitise(input)?;
        let digit_count = candidate.len() - usize::from(candidate.starts_with('+'));

        let parsed = phonenumber::parse(Some(region.id()), &candidate).map_err(|error| {
            match error {
                phonenumber::ParseError::InvalidCountryCode => MsisdnError::UnknownCountryCode {
                    digits: candidate.trim_start_matches('+').to_owned(),
                },
                // Every other variant upstream can return here is a length
                // complaint in some form: `NoNumber` (under three digits),
                // `TooShortAfterIdd`, `TooShortNsn`, `TooLong`, and a
                // `MalformedInteger` that `sanitise` has already made
                // unreachable by refusing every non-digit.
                _ => MsisdnError::BadLength { got: digit_count },
            }
        })?;

        let country_code = parsed.code().value();
        let national = parsed.national().to_string();
        let national_offset = 1 + country_code_digits(country_code);
        // Deliberately assembled rather than taken from `Mode::E164`: E.164
        // *is* `+` then the code then the national number, and building it
        // this way makes `national_offset` correct by construction instead of
        // by a slice that could panic. `e164_is_assembled_exactly_as_upstream_formats_it`
        // pins the two together.
        let e164 = format!("+{country_code}{national}");

        if country_code_digits(country_code) + national.len() > MAX_E164_DIGITS {
            return Err(MsisdnError::BadLength {
                got: national.len(),
            });
        }

        let meta = parsed.metadata(&DATABASE);
        let line_type = meta.map_or(LineType::Unallocated, |m| plan::line_type_of(m, &national));

        if line_type == LineType::Unallocated {
            if country_code == CM_COUNTRY_CODE && national.len() == 8 && !national.starts_with("88")
            {
                return Err(MsisdnError::LegacyEightDigit { digits: national });
            }
            return Err(
                if meta.is_some_and(|m| plan::length_is_possible(m, national.len())) {
                    MsisdnError::Unallocated { national }
                } else {
                    MsisdnError::BadLength {
                        got: national.len(),
                    }
                },
            );
        }

        Ok(Self {
            e164,
            national_offset,
            region: meta.and_then(|m| Region::from_metadata_id(m.id())),
            line_type,
        })
    }

    /// Parse a number an SMS can actually be delivered to, reading a bare
    /// national number as Cameroonian.
    ///
    /// Everything [`Msisdn::parse`] accepts, minus every line type
    /// [`LineType::is_addressable`] refuses. This is what the API boundary
    /// calls.
    ///
    /// ```
    /// use sms_msisdn::{Msisdn, MsisdnError, LineType};
    ///
    /// // Any input shape reaches the same canonical E.164 form.
    /// let m = Msisdn::parse_mobile("+237 677 12 34 56").unwrap();
    /// assert_eq!(m.as_e164(), "+237677123456");
    /// assert_eq!(m.national(), "677123456");
    ///
    /// // A fixed line parses under `Msisdn::parse`, but `parse_mobile`
    /// // refuses it — nothing can reach it by SMS.
    /// assert_eq!(
    ///     Msisdn::parse_mobile("+237222123456"),
    ///     Err(MsisdnError::NotAddressable {
    ///         national: "222123456".to_owned(),
    ///         line_type: LineType::FixedLine,
    ///     })
    /// );
    /// ```
    pub fn parse_mobile(input: &str) -> Result<Self, MsisdnError> {
        Self::parse_mobile_in(input, Region::CM)
    }

    /// Parse a number an SMS can actually be delivered to, reading a bare
    /// national number against `region`.
    ///
    /// ```
    /// use sms_msisdn::{Msisdn, Region};
    ///
    /// // The NANP cannot say whether a number is mobile, so refusing
    /// // anything but `LineType::Mobile` would refuse North America whole.
    /// let ca: Region = "CA".parse().unwrap();
    /// assert!(Msisdn::parse_mobile_in("647-555-1234", ca).is_ok());
    /// ```
    pub fn parse_mobile_in(input: &str, region: Region) -> Result<Self, MsisdnError> {
        let m = Self::parse_in(input, region)?;
        if m.line_type.is_addressable() {
            Ok(m)
        } else {
            Err(MsisdnError::NotAddressable {
                national: m.national().to_owned(),
                line_type: m.line_type,
            })
        }
    }

    /// The canonical E.164 form, country code and `+` included.
    #[must_use]
    pub fn as_e164(&self) -> &str {
        &self.e164
    }

    /// The national significant number, without the country code.
    ///
    /// Leading zeros are part of the national significant number in a handful
    /// of countries (Italy, Côte d'Ivoire, Benin, Congo-Brazzaville) and are
    /// kept.
    #[must_use]
    pub fn national(&self) -> &str {
        &self.e164[self.national_offset..]
    }

    /// The region this number belongs to, where the metadata names one.
    ///
    /// `None` for a number whose country code is non-geographic (`+800`
    /// universal freephone, `+979` universal premium rate, the satellite
    /// ranges) — those have a calling code but no country.
    #[must_use]
    pub const fn region(&self) -> Option<Region> {
        self.region
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
    /// Keeps the country code and the last two digits, whatever the length of
    /// either: `+237*******56`, `+1********71`.
    ///
    /// ```
    /// use sms_msisdn::Msisdn;
    ///
    /// assert_eq!(Msisdn::parse("677123456").unwrap().masked(), "+237*******56");
    /// assert_eq!(Msisdn::parse("+14155552671").unwrap().masked(), "+1********71");
    /// ```
    #[must_use]
    pub fn masked(&self) -> String {
        let national = self.national();
        let keep = 2.min(national.len());
        let hidden = national.len() - keep;
        format!(
            "{}{}{}",
            &self.e164[..self.national_offset],
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

/// How many digits an E.164 country calling code has. They are one to three.
const fn country_code_digits(code: u16) -> usize {
    if code >= 100 {
        3
    } else if code >= 10 {
        2
    } else {
        1
    }
}

/// Strip separators and reject anything that is not a digit or a leading `+`.
///
/// Returns the input as `phonenumber` should see it: digits, with the `+` kept
/// where there was one. This pass exists rather than handing the raw string
/// straight over because libphonenumber *helpfully* maps letters onto their
/// phone-keypad digits — `677abc456` parses upstream as `+237677222456`,
/// which is a different subscriber's number. Silently rewriting a recipient
/// is not a normalisation this gateway is willing to perform.
fn sanitise(input: &str) -> Result<String, MsisdnError> {
    let mut out = String::with_capacity(input.len() + 1);
    let mut plus = false;
    let mut digits = 0usize;
    for c in input.chars() {
        if SEPARATORS.contains(&c) {
            continue;
        }
        if c == '+' {
            // A `+` is only meaningful once, and only before any digit.
            if plus || digits > 0 {
                return Err(MsisdnError::InvalidCharacter { ch: c });
            }
            plus = true;
            out.push('+');
            continue;
        }
        if c.is_ascii_digit() {
            digits += 1;
            out.push(c);
        } else {
            return Err(MsisdnError::InvalidCharacter { ch: c });
        }
    }

    if digits == 0 {
        return Err(MsisdnError::Empty);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{LineType, Msisdn, MsisdnError, Region};

    fn region(code: &str) -> Region {
        code.parse().unwrap()
    }

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
    fn other_country_codes_are_parsed_rather_than_refused() {
        // This is the behaviour change: `+33…` used to be
        // `MsisdnError::UnsupportedCountry`, because this crate knew only
        // Cameroon. Every country is supported now, and which countries a
        // tenant may send to is a policy question above this crate.
        for input in ["+33612345678", "0033612345678"] {
            let m = Msisdn::parse(input).unwrap_or_else(|e| panic!("{input:?}: {e}"));
            assert_eq!(m.as_e164(), "+33612345678", "{input:?}");
            assert_eq!(m.region(), Some(region("FR")));
        }
    }

    #[test]
    fn an_international_prefix_other_than_plus_is_the_dialling_regions_own() {
        // Easy to get wrong, and it silently changes which country a number
        // resolves to: `00` is Cameroon's international prefix (and most of
        // Europe's and Africa's), but North America dials `011` and Nigeria
        // `009`. Pinned as a test because `parse`/`parse_in`'s own docs make
        // a claim about `00` that is only true region by region.
        assert_eq!(
            Msisdn::parse("0033612345678").unwrap().as_e164(),
            "+33612345678"
        );
        assert_eq!(
            Msisdn::parse_in("0033612345678", region("GB"))
                .unwrap()
                .as_e164(),
            "+33612345678"
        );
        // The same string dialled from the USA is not a French number, and
        // is not a valid US one either — `011 33 …` is what a US caller
        // would have dialled, and that does resolve.
        assert!(Msisdn::parse_in("0033612345678", region("US")).is_err());
        assert_eq!(
            Msisdn::parse_in("01133612345678", region("US"))
                .unwrap()
                .as_e164(),
            "+33612345678"
        );
        // A leading `+` is unambiguous from anywhere.
        for code in ["CM", "US", "GB", "NG"] {
            assert_eq!(
                Msisdn::parse_in("+33612345678", region(code))
                    .unwrap()
                    .as_e164(),
                "+33612345678",
                "{code}"
            );
        }
    }

    #[test]
    fn a_calling_code_no_country_uses_is_still_refused() {
        assert_eq!(
            Msisdn::parse("+999123456789"),
            Err(MsisdnError::UnknownCountryCode {
                digits: "999123456789".into()
            })
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
    fn the_eight_digit_hint_is_cameroon_only() {
        // Eight national digits is an ordinary, valid length in Denmark and
        // Norway, so the hint must not follow the number abroad.
        assert!(Msisdn::parse_mobile_in("20123456", region("DK")).is_ok());
        assert!(Msisdn::parse_mobile_in("41234567", region("NO")).is_ok());
        // And an eight-digit number that is invalid abroad is simply
        // invalid, never "add a 6 or a 2".
        assert!(!matches!(
            Msisdn::parse_in("00000000", region("DK")),
            Err(MsisdnError::LegacyEightDigit { .. })
        ));
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
        assert!(Msisdn::parse("641123456").is_ok());
    }

    #[test]
    fn the_642_block_follows_the_metadata_not_this_crates_old_table() {
        // This crate's hand-written plan claimed 640-642 were assigned;
        // libphonenumber 9.0.33 assigns 640 and 641 only. Following the
        // metadata is the whole point of the swap — a local override for one
        // prefix would reinstate exactly the drift it removes. Recorded as a
        // test rather than a comment so the day upstream changes its mind is
        // a visible failure, not a silent one.
        assert!(Msisdn::parse("640123456").is_ok());
        assert!(Msisdn::parse("641123456").is_ok());
        assert_eq!(
            Msisdn::parse("642123456"),
            Err(MsisdnError::Unallocated {
                national: "642123456".into()
            })
        );
    }

    #[test]
    fn fixed_lines_parse_but_do_not_pass_the_send_path() {
        let m = Msisdn::parse("+237222123456").unwrap();
        assert_eq!(m.line_type(), LineType::FixedLine);
        assert!(!m.is_addressable());
        assert_eq!(
            Msisdn::parse_mobile("+237222123456"),
            Err(MsisdnError::NotAddressable {
                national: "222123456".into(),
                line_type: LineType::FixedLine
            })
        );
    }

    #[test]
    fn a_cameroon_fixed_line_is_still_refused_now_that_fixed_or_mobile_is_accepted() {
        // The guard for decision 2: widening `parse_mobile` to accept
        // `FixedLineOrMobile` must not widen it to accept a country that
        // genuinely distinguishes the two. CM metadata returns `FixedLine`
        // for the whole 2(?:22|33) block, so every one of these stays out.
        for input in ["222123456", "233123456", "+237222000000", "237233999999"] {
            let parsed = Msisdn::parse(input).unwrap_or_else(|e| panic!("{input:?}: {e}"));
            assert_eq!(parsed.line_type(), LineType::FixedLine, "{input:?}");
            assert!(
                matches!(
                    Msisdn::parse_mobile(input),
                    Err(MsisdnError::NotAddressable {
                        line_type: LineType::FixedLine,
                        ..
                    })
                ),
                "{input:?} must not pass the send path"
            );
        }
    }

    #[test]
    fn north_america_is_addressable_even_though_it_is_not_mobile() {
        // The other half of decision 2, and the reason for it: the NANP does
        // not encode fixed-versus-mobile, so every US and Canadian number is
        // `FixedLineOrMobile`. Accepting only `Mobile` rejects the continent.
        for (input, expected) in [
            ("+14155552671", "+14155552671"),
            ("+12125550000", "+12125550000"),
            ("+16475551234", "+16475551234"),
        ] {
            let m = Msisdn::parse_mobile(input).unwrap_or_else(|e| panic!("{input:?}: {e}"));
            assert_eq!(m.as_e164(), expected);
            assert_eq!(m.line_type(), LineType::FixedLineOrMobile, "{input:?}");
            assert!(m.is_addressable());
        }
        // And read from a bare national number against an explicit region.
        assert_eq!(
            Msisdn::parse_mobile_in("(415) 555-2671", region("US"))
                .unwrap()
                .as_e164(),
            "+14155552671"
        );
        assert_eq!(
            Msisdn::parse_mobile_in("647-555-1234", region("CA"))
                .unwrap()
                .as_e164(),
            "+16475551234"
        );
    }

    #[test]
    fn the_reach_this_crate_now_has() {
        // E.164 normalisation and send-path acceptance for every market the
        // multi-country program names. Both forms per country: the number as
        // an integrator would send it, and the bare national number read
        // against that country's region.
        for (code, national, e164) in [
            ("FR", "06 12 34 56 78", "+33612345678"),
            ("US", "(415) 555-2671", "+14155552671"),
            ("CA", "647-555-1234", "+16475551234"),
            ("IN", "98765 43210", "+919876543210"),
            ("NG", "0803 123 4567", "+2348031234567"),
            ("DE", "0151 12345678", "+4915112345678"),
            ("GB", "07912 345678", "+447912345678"),
        ] {
            let from_national = Msisdn::parse_mobile_in(national, region(code))
                .unwrap_or_else(|e| panic!("{code} {national:?}: {e}"));
            assert_eq!(from_national.as_e164(), e164, "{code} from national");
            assert_eq!(from_national.region(), Some(region(code)), "{code} region");
            assert!(from_national.is_addressable(), "{code} addressable");

            // The same number as an integrator would actually send it, with
            // the Cameroonian default region still in force.
            let from_e164 =
                Msisdn::parse_mobile(e164).unwrap_or_else(|e| panic!("{code} {e164:?}: {e}"));
            assert_eq!(from_e164, from_national, "{code} e164 vs national");
        }
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
    fn letters_are_refused_rather_than_mapped_onto_keypad_digits() {
        // libphonenumber reads `abc` as `222`, so `677abc456` would parse
        // upstream as a real, different subscriber: +237677222456. Sending
        // an OTP to a number the caller never wrote is worse than a 422.
        assert!(Msisdn::parse("677abc456").is_err());
        assert!(Msisdn::parse("+1-800-FLOWERS").is_err());
    }

    #[test]
    fn an_absurdly_long_number_is_refused_by_the_e164_digit_cap() {
        assert_eq!(
            Msisdn::parse("1234567890123456"),
            Err(MsisdnError::BadLength { got: 16 })
        );
    }

    #[test]
    fn e164_length_fits_the_schema_column() {
        // Message.msisdn is @length(min: 8, max: 15). The floor used to be
        // 12 — `+237` plus nine digits — which refused Denmark, Norway and
        // Iceland, whose E.164 numbers are 11 characters.
        for input in ["+237677123456", "+23788123456", "+237222123456"] {
            let len = Msisdn::parse(input).unwrap().as_e164().len();
            assert!((8..=15).contains(&len), "{input} produced {len} characters");
        }
        // The countries the old floor excluded, and the longest E.164 form
        // this crate can produce, on both sides of the new bound.
        for input in ["+4520123456", "+4741234567", "+3546112345"] {
            let m = Msisdn::parse(input).unwrap_or_else(|e| panic!("{input:?}: {e}"));
            assert_eq!(m.as_e164().len(), 11, "{input}");
            assert!(m.is_addressable(), "{input}");
        }
        assert!(Msisdn::parse("+4915112345678").unwrap().as_e164().len() <= 15);
    }

    #[test]
    fn masking_keeps_the_country_code_and_two_digits_at_any_length() {
        // Cameroon's output is byte-identical to what it always was.
        assert_eq!(
            Msisdn::parse("677123456").unwrap().masked(),
            "+237*******56"
        );
        assert_eq!(Msisdn::parse("88123456").unwrap().masked(), "+237******56");
        // And the same rule now holds for a one- or two-digit country code.
        assert_eq!(
            Msisdn::parse("+14155552671").unwrap().masked(),
            "+1********71"
        );
        assert_eq!(
            Msisdn::parse("+4520123456").unwrap().masked(),
            "+45******56"
        );
        assert_eq!(
            Msisdn::parse("+919876543210").unwrap().masked(),
            "+91********10"
        );
    }

    #[test]
    fn e164_is_assembled_exactly_as_upstream_formats_it() {
        // `parse_in` builds `+{code}{national}` itself rather than calling
        // `Mode::E164`, so that `national_offset` is correct by construction.
        // This pins the two together, including for the countries whose
        // national significant number keeps a leading zero.
        for (code, input) in [
            ("CM", "677123456"),
            ("CM", "88123456"),
            ("US", "4155552671"),
            ("DE", "015112345678"),
            ("CG", "061234567"),
            ("BJ", "0195123456"),
            ("CI", "0123456789"),
            ("IT", "3123456789"),
        ] {
            let region = region(code);
            let ours =
                Msisdn::parse_in(input, region).unwrap_or_else(|e| panic!("{code} {input:?}: {e}"));
            let upstream = phonenumber::parse(Some(region.id()), input)
                .unwrap()
                .format()
                .mode(phonenumber::Mode::E164)
                .to_string();
            assert_eq!(ours.as_e164(), upstream, "{code} {input:?}");
            assert_eq!(
                ours.national(),
                &upstream[ours.national_offset..],
                "{code} {input:?} national offset"
            );
        }
    }

    #[test]
    fn round_trips_through_display_and_from_str() {
        let m: Msisdn = "677 12 34 56".parse().unwrap();
        assert_eq!(m.to_string(), "+237677123456");
        assert_eq!(m.to_string().parse::<Msisdn>().unwrap(), m);
    }
}
