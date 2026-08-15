Segment counting.

The concatenated limits are lower than the single-message ones because a
6-octet User Data Header is prepended to every part: 160 → 153 septets for
GSM-7, 70 → 67 UTF-16 code units for UCS-2.

Two-unit characters must not straddle a segment boundary — a GSM-7 escape
pair, or a UTF-16 surrogate pair — so segmentation is a packing loop rather
than a division. Getting this wrong undercounts by one on exactly the bodies
where the count matters.
