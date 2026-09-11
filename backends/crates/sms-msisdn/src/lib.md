E.164 normalisation for every country, with Cameroon (`+237`) as the default.

Numbers are normalised and validated against Google libphonenumber's own
metadata, embedded at compile time by the `phonenumber` crate — no runtime
file, no network, no hand-written national plan. Every number this crate
accepts normalises to `+` followed by a country code and that country's
national significant number, and every number it rejects is rejected
*synchronously*, at the API boundary — a Twilio-style `21614` failure
arriving three seconds later on a DLR is a strictly worse experience than a
`422`.

**Cameroon stays first.** [`Msisdn::parse`] and [`Msisdn::parse_mobile`] read
a bare national number as Cameroonian, so `677123456` still means
`+237677123456` with nothing to configure. [`Msisdn::parse_in`] and
[`Msisdn::parse_mobile_in`] take a [`Region`] explicitly for everyone else.

```
use sms_msisdn::{LineType, Msisdn, MsisdnError, Region};

// Anything a human might paste, read as Cameroonian by default.
let m = Msisdn::parse_mobile("(+237) 6 77 12 34 56").unwrap();
assert_eq!(m.as_e164(), "+237677123456");
assert_eq!(m.national(), "677123456");
assert_eq!(m.line_type(), LineType::Mobile);

// A country code on the number outranks the default region.
assert_eq!(Msisdn::parse("+33 6 12 34 56 78").unwrap().as_e164(), "+33612345678");

// An explicit region reads a bare national number that country's way.
let us: Region = "US".parse().unwrap();
assert_eq!(
    Msisdn::parse_mobile_in("(415) 555-2671", us).unwrap().as_e164(),
    "+14155552671",
);

// Cameroonian fixed lines parse, but cannot receive an SMS.
assert_eq!(Msisdn::parse("237222123456").unwrap().line_type(), LineType::FixedLine);
assert!(matches!(
    Msisdn::parse_mobile("237222123456"),
    Err(MsisdnError::NotAddressable { .. })
));

// Pre-2014 eight-digit Cameroonian numbers get their own error rather than
// a guess about whether the missing digit was a 6 or a 2.
assert!(matches!(
    Msisdn::parse("77123456"),
    Err(MsisdnError::LegacyEightDigit { .. })
));
```

# Addressability, not mobility

[`Msisdn::parse_mobile`] accepts [`LineType::Mobile`] **and**
[`LineType::FixedLineOrMobile`]. The North American Numbering Plan does not
encode the fixed/mobile distinction at all, so accepting only `Mobile` would
reject every US and Canadian number. Cameroon is unaffected: its metadata
returns `Mobile` for mobile ranges and `FixedLine` for `2xx`, so a
Cameroonian fixed line is still refused exactly as it always was. See
[`LineType::is_addressable`].

# Cost

`phonenumber` resolves its regexes through a shared cache behind a mutex, so
parsing is not lock-free. At this gateway's throughput that has never been
close to mattering; it is recorded here so nobody has to rediscover it if it
ever does.

Operator inference is deliberately **not** here — see
[`OperatorPrefixTable`].
