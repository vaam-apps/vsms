#![doc = include_str!("operator.md")]

use crate::Msisdn;

/// A prefix-to-operator table, built from the database at startup.
///
/// Lookup is longest-prefix-wins, so `655` beats `65` beats `6`. Ties between
/// equal-length prefixes go to whichever was inserted first.
#[derive(Debug, Clone, Default)]
pub struct OperatorPrefixTable {
    /// Sorted by descending prefix length; insertion order preserved within a
    /// length.
    entries: Vec<(String, String)>,
}

impl OperatorPrefixTable {
    /// Build a table from `(prefix, operator)` rows.
    ///
    /// Prefixes are matched against the **national** number, not the E.164
    /// form: `677`, not `+237677`. Rows whose prefix is not all ASCII digits
    /// are dropped — a malformed row should not shadow a valid one.
    pub fn new<I, P, O>(rows: I) -> Self
    where
        I: IntoIterator<Item = (P, O)>,
        P: Into<String>,
        O: Into<String>,
    {
        let mut entries: Vec<(String, String)> = rows
            .into_iter()
            .map(|(p, o)| (p.into(), o.into()))
            .filter(|(p, _)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            .collect();
        // Stable, so equal-length prefixes keep their insertion order.
        entries.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        Self { entries }
    }

    /// Whether the table holds no usable rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many rows the table holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Infer the operator for a number.
    ///
    /// `None` means no row matched — map it to `OperatorCode::unknown` and let
    /// routing fall through to a default route.
    #[must_use]
    pub fn lookup(&self, msisdn: &Msisdn) -> Option<&str> {
        self.lookup_national(msisdn.national())
    }

    /// Infer the operator from a bare national number.
    ///
    /// ```
    /// use sms_msisdn::OperatorPrefixTable;
    ///
    /// let table = OperatorPrefixTable::new([("6", "unknown"), ("67", "mtn"), ("655", "orange")]);
    ///
    /// // Longest prefix wins: "655" beats "6", even though "6" also matches.
    /// assert_eq!(table.lookup_national("655123456"), Some("orange"));
    /// // "67" beats the bare "6" fallback the same way.
    /// assert_eq!(table.lookup_national("677123456"), Some("mtn"));
    /// // No row starts this number at all.
    /// assert_eq!(table.lookup_national("222123456"), None);
    /// ```
    #[must_use]
    pub fn lookup_national(&self, national: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(prefix, _)| national.starts_with(prefix.as_str()))
            .map(|(_, operator)| operator.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> OperatorPrefixTable {
        OperatorPrefixTable::new([
            ("6", "unknown"),
            ("67", "mtn"),
            ("69", "orange"),
            ("655", "orange"),
            ("650", "mtn"),
        ])
    }

    #[test]
    fn longest_prefix_wins() {
        let t = table();
        assert_eq!(t.lookup_national("655123456"), Some("orange"));
        assert_eq!(t.lookup_national("650123456"), Some("mtn"));
        assert_eq!(t.lookup_national("677123456"), Some("mtn"));
        assert_eq!(t.lookup_national("699123456"), Some("orange"));
        assert_eq!(t.lookup_national("620123456"), Some("unknown"));
    }

    #[test]
    fn an_empty_table_infers_nothing() {
        let t = OperatorPrefixTable::default();
        assert!(t.is_empty());
        assert_eq!(t.lookup_national("677123456"), None);
    }

    #[test]
    fn malformed_rows_are_dropped_rather_than_shadowing_valid_ones() {
        let t = OperatorPrefixTable::new([("6xx", "junk"), ("", "junk"), ("67", "mtn")]);
        assert_eq!(t.len(), 1);
        assert_eq!(t.lookup_national("677123456"), Some("mtn"));
    }

    #[test]
    fn lookup_takes_the_national_number_not_the_e164_one() {
        let m = Msisdn::parse("+237677123456").unwrap();
        assert_eq!(table().lookup(&m), Some("mtn"));
    }
}
