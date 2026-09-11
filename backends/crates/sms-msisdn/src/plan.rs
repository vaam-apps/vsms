#![doc = include_str!("plan.md")]

use phonenumber::metadata::{Descriptor, Metadata};

use crate::{Msisdn, MsisdnError, Region};

/// What kind of line a number addresses.
///
/// The vocabulary is libphonenumber's, minus the kinds no E.164 number this
/// crate can produce ever reaches (`Emergency`, `ShortCode`, `StandardRate`,
/// `Carrier`, `NoInternational` — none is consulted by the resolver upstream
/// either) and plus [`LineType::Unallocated`], which upstream spells
/// `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineType {
    /// A mobile handset. Addressable.
    Mobile,
    /// A plan that does not distinguish fixed from mobile at all — every
    /// North American Numbering Plan number, and a long tail elsewhere.
    /// Addressable: see [`LineType::is_addressable`].
    FixedLineOrMobile,
    /// A fixed line, in a plan that says so. Reject at the API boundary.
    FixedLine,
    /// A toll-free number, such as Cameroon's `88x` range.
    TollFree,
    /// A premium-rate number.
    PremiumRate,
    /// A shared-cost number.
    SharedCost,
    /// A voice-over-IP number. Deliberately not addressable — see
    /// [`LineType::is_addressable`].
    Voip,
    /// A personal ("follow-me") number.
    PersonalNumber,
    /// A pager.
    Pager,
    /// A universal access number.
    Uan,
    /// A voicemail box.
    Voicemail,
    /// Reads as a number for its country, but matches no assigned range —
    /// Cameroon's `63x` and `642`–`649`, and the equivalent everywhere else.
    Unallocated,
}

impl LineType {
    /// Whether an SMS can be delivered here.
    ///
    /// **Addressability, not mobility, is the test**, and the difference is
    /// load-bearing: the North American Numbering Plan does not encode
    /// fixed-versus-mobile at all, so every US and Canadian number classifies
    /// as [`LineType::FixedLineOrMobile`]. Accepting only
    /// [`LineType::Mobile`] would reject North America outright.
    ///
    /// Cameroon is unaffected — its metadata returns `Mobile` for mobile
    /// ranges and `FixedLine` for `2xx`, so a Cameroonian fixed line is
    /// refused exactly as it always was.
    ///
    /// [`LineType::Voip`] is excluded on purpose. Plenty of voice-over-IP
    /// numbers do receive SMS (US Google Voice and Twilio numbers, for
    /// instance), so this is the conservative call rather than the obviously
    /// correct one, and it is the one to revisit first if a customer's
    /// traffic is being refused for no reason they recognise.
    ///
    /// ```
    /// use sms_msisdn::LineType;
    ///
    /// assert!(LineType::Mobile.is_addressable());
    /// assert!(LineType::FixedLineOrMobile.is_addressable());
    /// assert!(!LineType::FixedLine.is_addressable());
    /// assert!(!LineType::TollFree.is_addressable());
    /// ```
    #[must_use]
    pub const fn is_addressable(self) -> bool {
        matches!(self, LineType::Mobile | LineType::FixedLineOrMobile)
    }

    /// A short lowercase name, for logs and error payloads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            LineType::Mobile => "mobile",
            LineType::FixedLineOrMobile => "fixed_line_or_mobile",
            LineType::FixedLine => "fixed_line",
            LineType::TollFree => "toll_free",
            LineType::PremiumRate => "premium_rate",
            LineType::SharedCost => "shared_cost",
            LineType::Voip => "voip",
            LineType::PersonalNumber => "personal_number",
            LineType::Pager => "pager",
            LineType::Uan => "uan",
            LineType::Voicemail => "voicemail",
            LineType::Unallocated => "unallocated",
        }
    }
}

impl std::fmt::Display for LineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classify a number, reading a bare national number as Cameroonian.
///
/// `None` means the input could not be read as a number for that country at
/// all — the wrong number of digits, or nothing usable in it.
/// `Some(LineType::Unallocated)` is the distinct, narrower outcome: it reads
/// as a number, and is in no assigned range.
///
/// ```
/// use sms_msisdn::{classify, LineType};
///
/// // The ordinary, 9-digit mobile case.
/// assert_eq!(classify("677123456"), Some(LineType::Mobile));
///
/// // `88x` is the one legitimate 8-digit number in the plan — the short
/// // form of the shared-cost/toll-free range, not a truncated 9-digit one.
/// assert_eq!(classify("88123456"), Some(LineType::TollFree));
///
/// // `63x` is 9 digits and syntactically well-formed, but in no assigned
/// // block — a distinct outcome from both "wrong length" and "known but
/// // not mobile".
/// assert_eq!(classify("637123456"), Some(LineType::Unallocated));
///
/// // A pre-2014 eight-digit number is outside the plan entirely.
/// assert_eq!(classify("67712345"), None);
/// ```
#[must_use]
pub fn classify(number: &str) -> Option<LineType> {
    classify_in(number, Region::CM)
}

/// Classify a number, reading a bare national number against `region`.
///
/// See [`classify`] for what `None` means.
///
/// ```
/// use sms_msisdn::{classify_in, LineType, Region};
///
/// let us: Region = "US".parse().unwrap();
///
/// // The NANP does not distinguish fixed from mobile, and says so.
/// assert_eq!(classify_in("(415) 555-2671", us), Some(LineType::FixedLineOrMobile));
///
/// // A country code on the number outranks the region argument.
/// assert_eq!(classify_in("+237677123456", us), Some(LineType::Mobile));
/// ```
#[must_use]
pub fn classify_in(number: &str, region: Region) -> Option<LineType> {
    match Msisdn::parse_in(number, region) {
        Ok(parsed) => Some(parsed.line_type()),
        Err(MsisdnError::Unallocated { .. }) => Some(LineType::Unallocated),
        Err(_) => None,
    }
}

/// Resolve the line type of `national` against one country's metadata.
///
/// `national` must be the national significant number **with** any leading
/// zeros — that is the whole reason this exists rather than a call to
/// `PhoneNumber::number_type`; see the module doc.
pub(crate) fn line_type_of(meta: &Metadata, national: &str) -> LineType {
    let d = meta.descriptors();
    if !d.general().is_match(national) {
        return LineType::Unallocated;
    }

    // Upstream's own order, from `phonenumber::validator::number_type`.
    for (descriptor, line_type) in [
        (d.premium_rate(), LineType::PremiumRate),
        (d.toll_free(), LineType::TollFree),
        (d.shared_cost(), LineType::SharedCost),
        (d.voip(), LineType::Voip),
        (d.personal_number(), LineType::PersonalNumber),
        (d.pager(), LineType::Pager),
        (d.uan(), LineType::Uan),
        (d.voicemail(), LineType::Voicemail),
    ] {
        if descriptor.is_some_and(|x| x.is_match(national)) {
            return line_type;
        }
    }

    let fixed = d.fixed_line();
    let mobile = d.mobile();
    if fixed.is_some_and(|x| x.is_match(national)) {
        // A plan that gives fixed and mobile the identical pattern is a plan
        // that does not distinguish them — the NANP case.
        let same_pattern = fixed.map(|x| x.national_number().as_str())
            == mobile.map(|x| x.national_number().as_str());
        if same_pattern || mobile.is_some_and(|x| x.is_match(national)) {
            return LineType::FixedLineOrMobile;
        }
        return LineType::FixedLine;
    }
    if mobile.is_some_and(|x| x.is_match(national)) {
        return LineType::Mobile;
    }

    LineType::Unallocated
}

/// Whether `len` is a length any assigned range in this country uses.
///
/// This is what separates [`MsisdnError::BadLength`] (a truncated or
/// over-long number — the caller's data is wrong) from
/// [`MsisdnError::Unallocated`] (the right shape, no such range). The
/// per-type descriptors carry `possible_length`; the general one does not, so
/// the union of the typed ones is the only length signal the metadata offers.
///
/// `false` when no descriptor carries length data at all would be a lie, so
/// that case returns `true` — a length we cannot rule out is not a length we
/// get to reject.
pub(crate) fn length_is_possible(meta: &Metadata, len: usize) -> bool {
    let Ok(len) = u16::try_from(len) else {
        return false;
    };
    let d = meta.descriptors();
    let typed: [Option<&Descriptor>; 10] = [
        d.fixed_line(),
        d.mobile(),
        d.toll_free(),
        d.premium_rate(),
        d.shared_cost(),
        d.personal_number(),
        d.voip(),
        d.pager(),
        d.uan(),
        d.voicemail(),
    ];
    let mut any = false;
    for descriptor in typed.into_iter().flatten() {
        let lengths = descriptor.possible_length();
        if lengths.is_empty() {
            continue;
        }
        any = true;
        if lengths.contains(&len) {
            return true;
        }
    }
    !any
}

#[cfg(test)]
mod tests {
    use super::{LineType, classify, classify_in};
    use crate::Region;

    fn region(code: &str) -> Region {
        code.parse().unwrap()
    }

    #[test]
    fn assigned_mobile_prefixes() {
        // 642 is deliberately absent: libphonenumber 9.0.33 assigns 640 and
        // 641 only (`6(?:[25-9]\d|4[01])`), where this crate's own
        // hand-written table used to claim 640-642. See
        // `unassigned_mobile_prefixes_are_not_mobile` below.
        for p in [
            "620", "650", "660", "670", "680", "690", "699", "640", "641",
        ] {
            let n = format!("{p}123456");
            assert_eq!(classify(&n), Some(LineType::Mobile), "{n} should be mobile");
        }
    }

    #[test]
    fn unassigned_mobile_prefixes_are_not_mobile() {
        for p in ["630", "639", "642", "643", "649"] {
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
    fn only_mobile_and_fixed_or_mobile_are_addressable() {
        assert!(LineType::Mobile.is_addressable());
        assert!(LineType::FixedLineOrMobile.is_addressable());
        for t in [
            LineType::FixedLine,
            LineType::TollFree,
            LineType::PremiumRate,
            LineType::SharedCost,
            LineType::Voip,
            LineType::PersonalNumber,
            LineType::Pager,
            LineType::Uan,
            LineType::Voicemail,
            LineType::Unallocated,
        ] {
            assert!(!t.is_addressable(), "{t} must not be addressable");
        }
    }

    #[test]
    fn the_nanp_reports_fixed_line_or_mobile_because_it_cannot_tell() {
        for (code, number) in [("US", "(415) 555-2671"), ("CA", "647-555-1234")] {
            assert_eq!(
                classify_in(number, region(code)),
                Some(LineType::FixedLineOrMobile),
                "{number} in {code}"
            );
        }
    }

    #[test]
    fn a_country_code_on_the_number_outranks_the_region() {
        assert_eq!(
            classify_in("+237677123456", region("US")),
            Some(LineType::Mobile)
        );
        assert_eq!(classify_in("+14155552671", Region::CM), {
            Some(LineType::FixedLineOrMobile)
        });
    }

    #[test]
    fn a_national_number_with_a_leading_zero_still_classifies() {
        // Congo-Brazzaville, Benin and Cote d'Ivoire keep a leading zero in
        // the national significant number, which is exactly the case
        // `PhoneNumber::number_type` gets wrong upstream. See plan.md.
        for (code, number) in [
            ("CG", "061234567"),
            ("BJ", "0195123456"),
            ("CI", "0123456789"),
        ] {
            assert_eq!(
                classify_in(number, region(code)),
                Some(LineType::Mobile),
                "{number} in {code}"
            );
        }
    }
}
