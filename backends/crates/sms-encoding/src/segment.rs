//! Segment counting.
//!
//! The concatenated limits are lower than the single-message ones because a
//! 6-octet User Data Header is prepended to every part: 160 → 153 septets for
//! GSM-7, 70 → 67 UTF-16 code units for UCS-2.
//!
//! Two-unit characters must not straddle a segment boundary — a GSM-7 escape
//! pair, or a UTF-16 surrogate pair — so segmentation is a packing loop rather
//! than a division. Getting this wrong undercounts by one on exactly the bodies
//! where the count matters.

/// GSM-7 septets in a single, unconcatenated message.
pub const GSM7_SINGLE: usize = 160;
/// GSM-7 septets per part once the message is concatenated.
pub const GSM7_CONCATENATED: usize = 153;
/// UTF-16 code units in a single, unconcatenated message.
pub const UCS2_SINGLE: usize = 70;
/// UTF-16 code units per part once the message is concatenated.
pub const UCS2_CONCATENATED: usize = 67;

/// Result of packing a body into segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Packing {
    pub(crate) length: usize,
    pub(crate) segments: u8,
    pub(crate) per_segment: usize,
}

/// Pack per-character costs into segments without splitting a two-unit
/// character.
///
/// `segments` saturates at 255: the concatenation sequence number in the UDH is
/// a single octet, so 255 parts is the hard protocol ceiling. Nothing anywhere
/// near it will pass the API, where `Message.segments` is capped at 10.
pub(crate) fn pack(costs: &[usize], single: usize, concatenated: usize) -> Packing {
    let length: usize = costs.iter().sum();
    if length <= single {
        return Packing {
            length,
            segments: 1,
            per_segment: single,
        };
    }
    let mut segments: usize = 1;
    let mut used: usize = 0;
    for &cost in costs {
        if used + cost > concatenated {
            segments += 1;
            used = 0;
        }
        used += cost;
    }
    #[allow(clippy::cast_possible_truncation)]
    let segments = segments.min(u8::MAX as usize) as u8;
    Packing {
        length,
        segments,
        per_segment: concatenated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ones(n: usize) -> Vec<usize> {
        vec![1; n]
    }

    #[test]
    fn empty_body_is_one_segment() {
        let p = pack(&[], GSM7_SINGLE, GSM7_CONCATENATED);
        assert_eq!((p.length, p.segments, p.per_segment), (0, 1, GSM7_SINGLE));
    }

    #[test]
    fn exactly_one_segment_stays_single() {
        let p = pack(&ones(160), GSM7_SINGLE, GSM7_CONCATENATED);
        assert_eq!((p.segments, p.per_segment), (1, GSM7_SINGLE));
    }

    #[test]
    fn one_over_drops_to_the_concatenated_limit() {
        let p = pack(&ones(161), GSM7_SINGLE, GSM7_CONCATENATED);
        assert_eq!(
            (p.length, p.segments, p.per_segment),
            (161, 2, GSM7_CONCATENATED)
        );
        assert_eq!(pack(&ones(306), GSM7_SINGLE, GSM7_CONCATENATED).segments, 2);
        assert_eq!(pack(&ones(307), GSM7_SINGLE, GSM7_CONCATENATED).segments, 3);
    }

    #[test]
    fn a_gsm7_escape_pair_is_not_split_across_the_boundary() {
        // 152 single-septet characters, then a euro sign. The escape pair
        // cannot start at septet 153, so it moves whole into part two and part
        // one ends one septet short — which pushes the tail into a third part
        // that naive division does not see.
        let mut costs = ones(152);
        costs.push(2);
        costs.extend(ones(152));
        let p = pack(&costs, GSM7_SINGLE, GSM7_CONCATENATED);
        assert_eq!(p.length, 306);
        assert_eq!(
            306_usize.div_ceil(GSM7_CONCATENATED),
            2,
            "division would say two parts"
        );
        assert_eq!(p.segments, 3, "the straddle costs a third part");
    }

    #[test]
    fn a_ucs2_surrogate_pair_is_not_split_across_the_boundary() {
        let mut costs = ones(66);
        costs.push(2); // an emoji straddling the 67-unit boundary
        costs.extend(ones(66));
        let p = pack(&costs, UCS2_SINGLE, UCS2_CONCATENATED);
        assert_eq!(p.length, 134);
        assert_eq!(
            134_usize.div_ceil(UCS2_CONCATENATED),
            2,
            "division would say two parts"
        );
        assert_eq!(p.segments, 3);
    }

    #[test]
    fn segments_saturate_at_the_udh_ceiling() {
        let p = pack(&ones(153 * 300), GSM7_SINGLE, GSM7_CONCATENATED);
        assert_eq!(p.segments, u8::MAX);
    }
}
