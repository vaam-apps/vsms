Unconditional, lossless-to-the-recipient cleanup.

Everything replaced here is a typographic substitute for a character that is
already in GSM 03.38, or an invisible character that carries no meaning in an
SMS. Nobody reading the message on a handset can tell the difference, and a
single `’` arriving from a copy-paste out of Word is enough on its own to
push a 160-character body to UCS-2 and halve it to 70.

Anything *perceptible* — `ç` → `c`, `«` → `"` — is transliteration, is
opt-in per app, and lives in [`crate::translit`].
