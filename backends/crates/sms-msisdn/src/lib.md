E.164 normalisation for Cameroon (`+237`).

Cameroon has been a closed 9-digit plan since November 2014. Every number
this crate accepts normalises to `+237` followed by exactly the national
significant number, and every number it rejects is rejected *synchronously*,
at the API boundary — a Twilio-style `21614` failure arriving three seconds
later on a DLR is a strictly worse experience than a `422`.

```
use sms_msisdn::{LineType, Msisdn, MsisdnError};

// Anything a human might paste.
let m = Msisdn::parse_mobile("(+237) 6 77 12 34 56").unwrap();
assert_eq!(m.as_e164(), "+237677123456");
assert_eq!(m.national(), "677123456");
assert_eq!(m.line_type(), LineType::Mobile);

// Fixed lines parse, but cannot receive an SMS.
assert_eq!(Msisdn::parse("237222123456").unwrap().line_type(), LineType::FixedLine);
assert!(matches!(
    Msisdn::parse_mobile("237222123456"),
    Err(MsisdnError::NotMobile { .. })
));

// Pre-2014 eight-digit numbers get their own error rather than a guess.
assert!(matches!(
    Msisdn::parse("77123456"),
    Err(MsisdnError::LegacyEightDigit { .. })
));
```

Operator inference is deliberately **not** here — see
[`OperatorPrefixTable`].
