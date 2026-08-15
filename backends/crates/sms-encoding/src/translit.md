Opt-in, *perceptible* rewriting into GSM 03.38.

`ç` → `c` and `ê` → `e` are fine in a delivery notification and are
corruption in a name field, so this is per-app (`App.transliterateToGsm7`)
and never automatic. [`crate::normalise`] is what runs unconditionally.

Three passes, in order:

1. the normalisation substitutions, so a transliterated body is at least as
   good as a normalised one and [`transliterate_to_gsm7`] can be used on its
   own to produce the `suggestion` in an [`crate::EncodingReport`];
2. an explicit table for characters Unicode does not decompose — `œ`, the
   guillemets, a few symbols;
3. NFD, then drop combining marks. This is what turns `ç` into `c`, `ê`
   into `e` and `Ù` into `U`, and it covers the whole Latin supplement
   without a hand-written entry per character.

Characters that survive all three — CJK, emoji, `°` — are left untouched.
The message stays UCS-2 and the caller finds out, rather than receiving a
silently mangled body.
