Line-type classification, over Google libphonenumber's own metadata.

This module used to hold a hand-written Cameroon numbering plan. It now holds
a vocabulary ([`LineType`]) and a resolver that reads the numbering plan of
whichever country a number belongs to out of the metadata `phonenumber`
embeds at compile time, so every country is covered by data rather than by
240 hand-written block tables nobody would track ITU amendments for.

Cameroon's own plan is unchanged by that swap and worth keeping written down,
because it is the plan every test in this crate is pinned against:

```text
mobile      (?:24[23]|6(?:[25-9]\d|4[01]))\d{6}
fixed line  2(?:22|33)\d{6}
toll free   88\d{6,7}
general     [26]\d{8}|88\d{6,7}
```

Note what is *absent* from the mobile range: `63x` and `642`–`649` are not
assigned; `640` and `641` are. The `88x` toll-free range is the one
legitimate 8-digit number in an otherwise closed 9-digit plan, and Camtel
mobile (`242`/`243`) sits inside the leading-`2` range rather than the
leading-`6` one — a classifier that assumes "6 is mobile, 2 is fixed" gets
that wrong.

# Why this resolves the line type itself instead of calling `number_type`

`phonenumber::PhoneNumber::number_type` looks like exactly the right call and
is wrong for a measurable set of countries. It passes
`NationalNumber::value().to_string()` to the matcher, which **drops leading
zeros**, while `PhoneNumber::is_valid` passes the `Display` form, which keeps
them. For every country whose national significant number retains a leading
zero the two disagree, and `number_type` is the one that is wrong.

Measured, not assumed, against all 245 regions in the 9.0.33 metadata, over
every example number the metadata carries for mobile, fixed line, toll free,
premium rate and voice-over-IP — 919 numbers in total. [`line_type_of`] agrees with
`number_type` on 884, is strictly better on 15, and is never worse on a valid
number. Three of those 15 are the **mobile** ranges of Congo-Brazzaville,
Benin and Côte d'Ivoire — Cameroon's neighbours, whose handsets
`number_type` reports as unclassifiable and this resolver reports correctly.

The precedence below is upstream's own, from `validator::number_type`
(premium rate, toll free, shared cost, voice-over-IP, personal number, pager, UAN,
voicemail, then the fixed/mobile pair), reproduced rather than reinvented.
The only deliberate deviation is the string it matches against.

Two residual gaps, both known and both harmless here: a number whose country
code is shared between regions *and* whose national number keeps a leading
zero resolves no metadata at all (Italian and Vatican fixed lines are the
only instances in the sweep), and those land as [`LineType::Unallocated`].
Neither is addressable by SMS either way, so nothing this crate gates on
changes. Italian *mobiles* carry no leading zero and classify correctly.
