// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

/// Computes the exclusive upper bound for keys sharing a storage-key prefix.
///
/// Trailing `0xff` bytes are omitted after incrementing the last remaining
/// byte, so shorter keys outside the prefix are excluded from the range.
/// Returns `None` when the prefix is empty or all bytes are `0xff`.
pub fn to_key_prefix_iter_upper_bound(key_prefix: &[u8]) -> Option<Vec<u8>> {
    let last = key_prefix.iter().rposition(|byte| *byte != u8::MAX)?;
    let mut upper_bound = key_prefix[..=last].to_vec();
    upper_bound[last] += 1;
    Some(upper_bound)
}

#[cfg(test)]
mod tests {
    use super::to_key_prefix_iter_upper_bound;

    #[test]
    fn computes_prefix_upper_bound() {
        let cases: &[(&[u8], Option<&[u8]>)] = &[
            (&[], None),
            (&[0x12, 0x34], Some(&[0x12, 0x35])),
            (&[0x12, 0xff, 0xff], Some(&[0x13])),
            (&[0xff, 0xff], None),
        ];

        for &(prefix, expected) in cases {
            assert_eq!(
                to_key_prefix_iter_upper_bound(prefix).as_deref(),
                expected,
                "unexpected upper bound for prefix {prefix:?}",
            );
        }
    }
}
