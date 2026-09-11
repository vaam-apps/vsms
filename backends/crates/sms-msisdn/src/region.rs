#![doc = include_str!("region.md")]

use std::str::FromStr;

use crate::MsisdnError;

/// An ISO 3166-1 alpha-2 region, used to read a bare national number.
///
/// Construct one with [`FromStr`], or use [`Region::CM`]. The wrapped value is
/// private on purpose — see the module doc for why this is not a re-export of
/// `phonenumber::country::Id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Region(phonenumber::country::Id);

impl Region {
    /// Cameroon, `+237` — this crate's default, and the region every signature
    /// that does not take one explicitly uses.
    pub const CM: Self = Self(phonenumber::country::Id::CM);

    /// The uppercase alpha-2 code, e.g. `"CM"`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    /// The wrapped metadata id, for this crate's own parsing only.
    pub(crate) const fn id(self) -> phonenumber::country::Id {
        self.0
    }

    /// Build a region from a metadata region id (`Metadata::id`).
    ///
    /// That id is an alpha-2 code for every geographic entry and `"001"` for
    /// the non-geographic ones (shared-cost `+979`, satellite `+881`, …),
    /// hence the `Option` rather than an infallible conversion.
    pub(crate) fn from_metadata_id(id: &str) -> Option<Self> {
        phonenumber::country::Id::from_str(id).ok().map(Self)
    }
}

impl FromStr for Region {
    type Err = MsisdnError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        let unknown = || MsisdnError::UnknownRegion {
            code: trimmed.to_owned(),
        };
        if trimmed.len() != 2 || !trimmed.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(unknown());
        }
        Self::from_metadata_id(&trimmed.to_ascii_uppercase()).ok_or_else(unknown)
    }
}

impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::Region;
    use crate::MsisdnError;

    #[test]
    fn alpha_two_codes_round_trip_case_insensitively() {
        for (input, expected) in [("CM", "CM"), ("cm", "CM"), ("Fr", "FR"), (" us ", "US")] {
            let region: Region = input.parse().unwrap_or_else(|e| panic!("{input:?}: {e}"));
            assert_eq!(region.as_str(), expected, "{input:?}");
            assert_eq!(region.to_string(), expected, "{input:?}");
        }
    }

    #[test]
    fn cameroon_is_the_named_default() {
        assert_eq!(Region::CM, "CM".parse::<Region>().unwrap());
    }

    #[test]
    fn anything_that_is_not_a_region_is_refused_at_the_boundary() {
        for input in ["", "C", "CMR", "Cameroon", "C1", "237", "zz"] {
            assert!(
                matches!(
                    input.parse::<Region>(),
                    Err(MsisdnError::UnknownRegion { .. })
                ),
                "{input:?} should not be a region"
            );
        }
    }
}
