The region a bare national number is read against.

[`Msisdn::parse`][crate::Msisdn::parse] defaults to [`Region::CM`] so a bare
`677123456` still means `+237677123456`, exactly as it did when this crate
knew about no other country. [`Msisdn::parse_in`][crate::Msisdn::parse_in]
takes the region explicitly, which is the seam a later stage threads
`App.defaultCountry` through.

A region only matters for input that carries no country code of its own: a
leading `+` names the country itself and the region is then ignored.
`Msisdn::parse_in("+33612345678", Region::CM)` is a French number, not an
error.

An international prefix that is *not* `+` is the dialling region's own, so
the region still matters for it: `00` for Cameroon, most of Europe and much
of Africa, `011` for North America, `009` for Nigeria. See
[`Msisdn::parse_in`][crate::Msisdn::parse_in].

# Why this is a newtype and not a re-export

`phonenumber::country::Id` is the value this wraps, and re-exporting it
directly would have been one line. It is deliberately not re-exported:
`sms-msisdn` is one of this repository's pure crates, and a dependency's type
in a pure crate's public API makes every caller's signature name that
dependency too. A `Region` in a later `App` projection, or in a `sms-routing`
predicate, would pin those crates to `phonenumber`'s major version for a
value that is really just two ASCII letters.

The cost is one `FromStr` call at the boundary, and that constructor is the
only way in — so an invalid region code is refused at the edge rather than
carried inward.

```
use sms_msisdn::{MsisdnError, Region};

// ISO 3166-1 alpha-2, case-insensitive on the way in.
assert_eq!("US".parse::<Region>().unwrap().as_str(), "US");
assert_eq!("fr".parse::<Region>().unwrap(), "FR".parse::<Region>().unwrap());

// Cameroon is the default everywhere a region is not given.
assert_eq!(Region::CM.as_str(), "CM");

// Anything that is not a region is refused here, not three layers in.
assert!(matches!(
    "Cameroon".parse::<Region>(),
    Err(MsisdnError::UnknownRegion { .. })
));
```
