// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

/// Computes an exclusive upper bound for a storage-key prefix by incrementing
/// the prefix as a fixed-width big-endian byte sequence.
///
/// Returns `None` when the prefix is empty or all bytes are `0xff`.
pub fn to_key_prefix_iter_upper_bound(key_prefix: &[u8]) -> Option<Vec<u8>> {
    let mut upper_bound_excl_value = key_prefix.to_vec();
    if upper_bound_excl_value.len() == 0 {
        None
    } else {
        let mut carry = 1;
        let len = upper_bound_excl_value.len();
        for i in 0..len {
            if upper_bound_excl_value[len - 1 - i] == 255 {
                upper_bound_excl_value[len - 1 - i] = 0;
            } else {
                upper_bound_excl_value[len - 1 - i] += 1;
                carry = 0;
                break;
            }
        }
        // all bytes in lower_bound_incl are 255, which means no upper bound
        // is needed.
        if carry == 1 {
            None
        } else {
            Some(upper_bound_excl_value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::to_key_prefix_iter_upper_bound;

    #[test]
    fn computes_fixed_width_prefix_upper_bound() {
        let cases: &[(&[u8], Option<&[u8]>)] = &[
            (&[], None),
            (&[0x12, 0x34], Some(&[0x12, 0x35])),
            (&[0x12, 0xff, 0xff], Some(&[0x13, 0x00, 0x00])),
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
