GSM 03.38 vs UCS-2 analysis for SMS bodies.

The conventional wisdom about French text and SMS encoding is wrong in both
directions, and both errors cost money:

- **`é è à ù ì ò` are in the GSM 03.38 default alphabet.** One septet each.
  French accents do not, on their own, force UCS-2.
- **`ç` is not.** Only the uppercase `Ç` exists. So *"Votre code a été
  reçu"* silently drops from 160 characters to 70 and doubles the bill.
  Same for `ê â î ô û ë ï ÿ œ È À Ù Ê` — and for the typographic apostrophe
  `’`, which arrives constantly from Word and Google Docs and is invisible
  in review.
- **`€ [ ] { } \ | ^ ~` cost two septets each**, being in the extension
  table.

# Usage

```
use sms_encoding::{analyse, normalise, SmsEncoding};

// A typographic apostrophe alone would force UCS-2.
let raw = "Votre code d\u{2019}accès est 4821";
assert_eq!(analyse(raw).encoding, SmsEncoding::Ucs2);

// Normalisation is unconditional and imperceptible, and fixes it.
let body = normalise(raw);
let report = analyse(&body);
assert_eq!(report.encoding, SmsEncoding::Gsm7);
assert_eq!(report.segments, 1);

// `ç` needs transliteration, which is opt-in per app.
let report = analyse(&normalise("Votre code a été reçu"));
assert_eq!(report.encoding, SmsEncoding::Ucs2);
assert_eq!(report.offending[0].ch, 'ç');
assert_eq!(report.suggestion.as_deref(), Some("Votre code a été recu"));
```
