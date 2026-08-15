#![doc = include_str!("lib.md")]

use thiserror::Error;

pub mod password;

/// The separator, which is also the sentinel.
pub const SEPARATOR: char = ' ';

/// The encoding of an empty collection: one separator, not the empty string.
///
/// Matches the `SET DEFAULT ' '` applied to every multi-value column in
/// `0002_bootstrap`.
pub const EMPTY: &str = " ";

/// A value that cannot be represented in a sentinel-delimited column.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PackError {
    /// The value contains the separator, so packing it would silently split
    /// it into two on the way back out.
    ///
    /// Not reachable from any legal input: OAuth scope tokens and grant types
    /// exclude whitespace by grammar (RFC 6749 §3.3 makes the space the
    /// delimiter), and a redirect URI must percent-encode one. It is an error
    /// rather than an escape because a caller that hits it has a bug that
    /// silent escaping would hide.
    #[error("value {0:?} contains the separator and cannot be packed")]
    ContainsSeparator(String),

    /// The value is empty, which would pack to two adjacent separators and
    /// unpack to nothing — a value that vanishes.
    #[error("an empty value cannot be packed")]
    Empty,
}

/// Encode values into the sentinel-delimited form.
///
/// Returns [`EMPTY`] for an empty iterator. Order is preserved; duplicates are
/// not removed, because the column is a list and deduplication is a decision
/// for whoever owns the list, not for the encoding.
///
/// ```
/// # use sms_core::{pack, EMPTY};
/// assert_eq!(pack(["sms:send", "sms:read"]).unwrap(), " sms:send sms:read ");
/// assert_eq!(pack(Vec::<String>::new()).unwrap(), EMPTY);
/// ```
pub fn pack<I, S>(values: I) -> Result<String, PackError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut packed = String::from(EMPTY);

    for value in values {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(PackError::Empty);
        }
        if value.contains(SEPARATOR) {
            return Err(PackError::ContainsSeparator(value.to_owned()));
        }
        packed.push_str(value);
        packed.push(SEPARATOR);
    }

    Ok(packed)
}

/// Decode a sentinel-delimited column back into its values.
///
/// Tolerant on the way in by design: it accepts the sentinel form, the
/// unsentinelled form, repeated separators and the empty string, all of which
/// yield the same values. Reading is where hand-written SQL, a fixture or an
/// older row shows up, and refusing those buys nothing — [`pack`] is where the
/// invariant is enforced.
///
/// ```
/// # use sms_core::unpack;
/// assert_eq!(unpack(" sms:send sms:read "), ["sms:send", "sms:read"]);
/// assert_eq!(unpack(" "), Vec::<&str>::new());
/// ```
#[must_use]
pub fn unpack(packed: &str) -> Vec<&str> {
    packed.split_whitespace().collect()
}

/// Whether `value` is one of the packed values.
///
/// Use this rather than `packed.contains(value)`, which matches substrings of
/// neighbouring values — the whole reason the sentinels exist.
///
/// ```
/// # use sms_core::contains;
/// assert!(contains(" sms:send ", "sms:send"));
/// assert!(!contains(" sms:sendall ", "sms:send"));
/// ```
#[must_use]
pub fn contains(packed: &str, value: &str) -> bool {
    !value.is_empty() && unpack(packed).contains(&value)
}

/// The `.contains(...)` argument that tests membership of `value` in a
/// sentinel-delimited column.
///
/// The database-side counterpart to [`contains`]: filter builders expose
/// `.contains`, so a membership predicate has to be spelled as a substring
/// match, and it is only correct with the separators attached.
///
/// ```
/// # use sms_core::needle;
/// # use sms_core::contains;
/// assert_eq!(needle("sms:send"), " sms:send ");
/// // What the database sees is exactly what `contains` decides in Rust.
/// assert!(" sms:send sms:read ".contains(&needle("sms:send")));
/// assert!(!" sms:sendall ".contains(&needle("sms:send")));
/// ```
#[must_use]
pub fn needle(value: &str) -> String {
    format!("{SEPARATOR}{value}{SEPARATOR}")
}

#[cfg(test)]
mod tests {
    use super::{EMPTY, PackError, contains, needle, pack, unpack};

    #[test]
    fn packing_adds_both_sentinels() {
        assert_eq!(pack(["a", "b"]).unwrap(), " a b ");
    }

    #[test]
    fn an_empty_collection_is_one_separator_not_an_empty_string() {
        // The `SET DEFAULT ' '` in 0002_bootstrap has to agree with this, or
        // a row created by the database and a row created by Rust disagree
        // about what "no scopes" looks like.
        assert_eq!(pack(Vec::<&str>::new()).unwrap(), EMPTY);
        assert_eq!(unpack(EMPTY), Vec::<&str>::new());
    }

    #[test]
    fn pack_and_unpack_round_trip() {
        let values = ["sms:send", "sms:read", "provider:update"];
        assert_eq!(unpack(&pack(values).unwrap()), values);
    }

    #[test]
    fn a_value_containing_the_separator_is_refused() {
        // Silently escaping it would let a caller store one value and read
        // back two, which is worse than a loud failure at the boundary.
        assert_eq!(
            pack(["sms:send", "two words"]),
            Err(PackError::ContainsSeparator("two words".to_owned()))
        );
    }

    #[test]
    fn an_empty_value_is_refused() {
        assert_eq!(pack(["a", ""]), Err(PackError::Empty));
    }

    #[test]
    fn unpack_tolerates_forms_pack_would_never_emit() {
        assert_eq!(unpack(""), Vec::<&str>::new());
        assert_eq!(unpack("a b"), ["a", "b"]);
        assert_eq!(unpack("  a   b  "), ["a", "b"]);
    }

    #[test]
    fn membership_does_not_match_a_longer_neighbour() {
        // The entire reason the sentinels exist. A naive
        // `" sms:sendall ".contains("sms:send")` is true.
        assert!(" sms:sendall ".contains("sms:send"));
        assert!(!contains(" sms:sendall ", "sms:send"));
        assert!(contains(" sms:sendall ", "sms:sendall"));
    }

    #[test]
    fn membership_of_an_empty_value_is_never_true() {
        assert!(!contains(" a b ", ""));
        assert!(!contains(EMPTY, ""));
    }

    #[test]
    fn the_sql_needle_and_the_rust_predicate_agree() {
        // If these ever diverge, a policy check in Rust and the same check
        // pushed into a `where_expr` would disagree about the same row.
        let packed = pack(["sms:send", "sms:read"]).unwrap();
        for candidate in ["sms:send", "sms:read", "sms:sendall", "sms", "write"] {
            assert_eq!(
                packed.contains(&needle(candidate)),
                contains(&packed, candidate),
                "disagreement on {candidate:?}"
            );
        }
    }
}
