use rust_decimal::Decimal;

/// What a provider can do. Routing reads this, not the provider's identity —
/// a route should never special-case `if key == "orange_cm"` when it could
/// ask `if capabilities.ucs2` instead, because the second version stays
/// correct when a second UCS-2-capable provider shows up.
// Four booleans, matching the schema's own `Provider` model field for field
// (`supportsDlr`/`supportsAlphaSender`/`supportsUcs2`/`supportsConcat`) —
// they're independent feature flags, not states of one thing, so collapsing
// them into an enum would fight the shape this maps onto rather than
// clarify anything.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capabilities {
    /// Whether this provider pushes delivery receipts at all. A provider
    /// without this is permanently `submitted`-or-nothing — routing must not
    /// promise a caller a DLR that will never arrive.
    pub dlr: bool,
    /// Whether an unregistered alphanumeric sender ID is accepted, as
    /// opposed to requiring a pre-approved numeric short code.
    pub alphanumeric_sender: bool,
    /// Whether UCS-2 (non-GSM-7) bodies are accepted at all, as opposed to
    /// silently mangling or rejecting them.
    pub ucs2: bool,
    /// Whether a body spanning multiple segments is concatenated back
    /// together on the handset, as opposed to arriving as N separate texts.
    pub concatenation: bool,
    /// The submission rate this provider's *contract* allows, not a
    /// technical limit — Orange's is 5.0, not a number this crate measured.
    /// A `f64` because the source is a rate a human negotiated, not a count;
    /// see [`Provider.maxTps`](../../../docs/architecture.md) in the schema.
    pub tps_ceiling: f64,
    /// What one segment costs on this provider, in XAF. `Decimal`, never a
    /// float — this is money.
    pub cost_per_segment_xaf: Decimal,
}

#[cfg(test)]
mod tests {
    use super::Capabilities;
    use rust_decimal::Decimal;

    /// Nothing exercises the fields yet — this crate has no logic of its
    /// own to test beyond "the type exists and is what routing needs." The
    /// real test is `sms-provider-orange-cm` reporting the right values.
    #[test]
    fn is_copy_and_carries_every_field_routing_needs() {
        let capabilities = Capabilities {
            dlr: true,
            alphanumeric_sender: true,
            ucs2: true,
            concatenation: false,
            tps_ceiling: 5.0,
            cost_per_segment_xaf: Decimal::new(18, 0),
        };
        let copied = capabilities;
        assert_eq!(capabilities, copied);
    }
}
