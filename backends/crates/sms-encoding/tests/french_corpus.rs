//! Milestone 0's gate: correct on a French corpus, including `ç` and `’`.
//!
//! Every body here is one an app in Cameroon would plausibly send. The point of
//! the table is that the answers are *not* guessable from the character count —
//! two bodies of identical length land in different encodings, and the
//! difference is one cedilla.

use sms_encoding::{SmsEncoding, analyse, normalise, transliterate_to_gsm7};

struct Case {
    body: &'static str,
    /// Encoding of the body exactly as given.
    raw: SmsEncoding,
    /// Encoding after the unconditional normalisation pass.
    normalised: SmsEncoding,
    /// Encoding after opt-in transliteration on top of that.
    transliterated: SmsEncoding,
}

const CORPUS: &[Case] = &[
    Case {
        body: "Votre code de vérification est 4821. Il expire dans 5 minutes.",
        raw: SmsEncoding::Gsm7,
        normalised: SmsEncoding::Gsm7,
        transliterated: SmsEncoding::Gsm7,
    },
    Case {
        // The headline case. One cedilla, half the message.
        body: "Votre paiement de 5 000 FCFA a été reçu. Merci.",
        raw: SmsEncoding::Ucs2,
        normalised: SmsEncoding::Ucs2,
        transliterated: SmsEncoding::Gsm7,
    },
    Case {
        // The invisible case. A Word apostrophe and nothing else.
        body: "Bienvenue sur l\u{2019}application Vymalo.",
        raw: SmsEncoding::Ucs2,
        normalised: SmsEncoding::Gsm7,
        transliterated: SmsEncoding::Gsm7,
    },
    Case {
        // Accents everywhere, all of them in the default alphabet.
        body: "Vous avez déjà été crédité de 10 000 FCFA à l'agence de Bonabéri.",
        raw: SmsEncoding::Gsm7,
        normalised: SmsEncoding::Gsm7,
        transliterated: SmsEncoding::Gsm7,
    },
    Case {
        // Uppercase à is not in the alphabet even though lowercase à is.
        body: "À BIENTÔT",
        raw: SmsEncoding::Ucs2,
        normalised: SmsEncoding::Ucs2,
        transliterated: SmsEncoding::Gsm7,
    },
    Case {
        // Guillemets are perceptible, so normalisation leaves them alone and
        // only the opt-in pass rescues the body.
        body: "Répondez «STOP» pour ne plus recevoir nos messages.",
        raw: SmsEncoding::Ucs2,
        normalised: SmsEncoding::Ucs2,
        transliterated: SmsEncoding::Gsm7,
    },
    Case {
        // Nothing rescues an emoji.
        body: "Merci pour votre commande 🙂",
        raw: SmsEncoding::Ucs2,
        normalised: SmsEncoding::Ucs2,
        transliterated: SmsEncoding::Ucs2,
    },
];

#[test]
fn corpus_encodings_are_as_expected_at_each_stage() {
    for case in CORPUS {
        assert_eq!(
            analyse(case.body).encoding,
            case.raw,
            "raw: {:?}",
            case.body
        );

        let normalised = normalise(case.body);
        assert_eq!(
            analyse(&normalised).encoding,
            case.normalised,
            "normalised: {:?} -> {normalised:?}",
            case.body
        );

        let (transliterated, _) = transliterate_to_gsm7(&normalised);
        assert_eq!(
            analyse(&transliterated).encoding,
            case.transliterated,
            "transliterated: {:?} -> {transliterated:?}",
            case.body
        );
    }
}

#[test]
fn normalisation_never_makes_a_body_worse() {
    for case in CORPUS {
        let before = analyse(case.body);
        let after = analyse(&normalise(case.body));
        assert!(
            after.segments <= before.segments,
            "{:?}: {} segments became {}",
            case.body,
            before.segments,
            after.segments
        );
    }
}

#[test]
fn a_suggestion_is_offered_exactly_when_transliteration_would_help() {
    for case in CORPUS {
        let report = analyse(case.body);
        let helps = case.raw == SmsEncoding::Ucs2 && case.transliterated == SmsEncoding::Gsm7;
        assert_eq!(
            report.suggestion.is_some(),
            helps,
            "{:?} suggestion was {:?}",
            case.body,
            report.suggestion
        );
        if let Some(s) = report.suggestion {
            assert_eq!(analyse(&s).encoding, SmsEncoding::Gsm7);
        }
    }
}

#[test]
fn the_cedilla_is_worth_exactly_one_segment_on_a_full_length_body() {
    // 148 characters: comfortably one GSM-7 segment, and comfortably three
    // UCS-2 ones.
    let clean = "Votre commande a ete enregistree et sera livree demain entre 8h et 12h a l'adresse indiquee. Merci de votre confiance et a tres bientot chez Vymalo.";
    let dirty = clean
        .replace("enregistree", "enregistrée")
        .replace("livree", "livrée");
    let cedilla = clean.replace("confiance", "confiançe");

    let a = analyse(clean);
    assert_eq!((a.encoding, a.segments), (SmsEncoding::Gsm7, 1));

    // Accents alone: still one segment.
    let b = analyse(&dirty);
    assert_eq!((b.encoding, b.segments), (SmsEncoding::Gsm7, 1));

    // A single cedilla: UCS-2, and the same text now costs three segments.
    let c = analyse(&cedilla);
    assert_eq!(c.encoding, SmsEncoding::Ucs2);
    assert_eq!(c.segments, 3);
    assert_eq!(c.offending.len(), 1);
    assert_eq!(c.offending[0].ch, 'ç');
}

#[test]
fn offending_offsets_index_the_body_that_was_analysed() {
    for case in CORPUS {
        for offending in analyse(case.body).offending {
            assert_eq!(
                case.body[offending.offset..].chars().next(),
                Some(offending.ch),
                "{:?} reported {:?} at byte {}",
                case.body,
                offending.ch,
                offending.offset
            );
        }
    }
}
