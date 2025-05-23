// This file is derived from revm (MIT licensed)
// Copyright (c) 2021-2025 draganrakita
// Modified by Conflux Foundation 2025

use super::{
    consts::{
        FP_LENGTH, G2_INPUT_ITEM_LENGTH, G2_OUTPUT_LENGTH, PADDED_FP_LENGTH,
    },
    utils::{fp_from_bendian, fp_to_bytes, remove_padding},
};
use blst::{
    blst_fp2, blst_p2_affine, blst_p2_affine_in_g2, blst_p2_affine_on_curve,
};

/// Encodes a G2 point in affine format into byte slice with padded elements.
pub(super) fn encode_g2_point(input: &blst_p2_affine) -> Vec<u8> {
    let mut out = vec![0u8; G2_OUTPUT_LENGTH];
    fp_to_bytes(&mut out[..PADDED_FP_LENGTH], &input.x.fp[0]);
    fp_to_bytes(
        &mut out[PADDED_FP_LENGTH..2 * PADDED_FP_LENGTH],
        &input.x.fp[1],
    );
    fp_to_bytes(
        &mut out[2 * PADDED_FP_LENGTH..3 * PADDED_FP_LENGTH],
        &input.y.fp[0],
    );
    fp_to_bytes(
        &mut out[3 * PADDED_FP_LENGTH..4 * PADDED_FP_LENGTH],
        &input.y.fp[1],
    );
    out.into()
}

/// Convert the following field elements from byte slices into a
/// `blst_p2_affine` point.
pub(super) fn decode_and_check_g2(
    x1: &[u8; 48], x2: &[u8; 48], y1: &[u8; 48], y2: &[u8; 48],
) -> Result<blst_p2_affine, String> {
    Ok(blst_p2_affine {
        x: check_canonical_fp2(x1, x2)?,
        y: check_canonical_fp2(y1, y2)?,
    })
}

/// Checks whether or not the input represents a canonical fp2 field element,
/// returning the field element if successful.
pub(super) fn check_canonical_fp2(
    input_1: &[u8; 48], input_2: &[u8; 48],
) -> Result<blst_fp2, String> {
    let fp_1 = fp_from_bendian(input_1)?;
    let fp_2 = fp_from_bendian(input_2)?;

    let fp2 = blst_fp2 { fp: [fp_1, fp_2] };

    Ok(fp2)
}

/// Extracts a G2 point in Affine format from a 256 byte slice representation.
///
/// **Note**: This function will perform a G2 subgroup check if `subgroup_check`
/// is set to `true`.
pub(super) fn extract_g2_input(
    input: &[u8], subgroup_check: bool,
) -> Result<blst_p2_affine, String> {
    if input.len() != G2_INPUT_ITEM_LENGTH {
        return Err(format!(
            "Input should be {G2_INPUT_ITEM_LENGTH} bytes, was {}",
            input.len()
        ));
    }

    let mut input_fps = [&[0; FP_LENGTH]; 4];
    for i in 0..4 {
        input_fps[i] = remove_padding(
            &input[i * PADDED_FP_LENGTH..(i + 1) * PADDED_FP_LENGTH],
        )?;
    }

    let out = decode_and_check_g2(
        input_fps[0],
        input_fps[1],
        input_fps[2],
        input_fps[3],
    )?;

    if subgroup_check {
        // NB: Subgroup checks
        //
        // Scalar multiplications, MSMs and pairings MUST perform a subgroup
        // check.
        //
        // Implementations SHOULD use the optimized subgroup check method:
        //
        // https://eips.ethereum.org/assets/eip-2537/fast_subgroup_checks
        //
        // On any input that fail the subgroup check, the precompile MUST return
        // an error.
        //
        // As endomorphism acceleration requires input on the correct subgroup,
        // implementers MAY use endomorphism acceleration.
        if unsafe { !blst_p2_affine_in_g2(&out) } {
            return Err("Element not in G2".to_string());
        }
    } else {
        // From EIP-2537:
        //
        // Error cases:
        //
        // * An input is neither a point on the G2 elliptic curve nor the
        //   infinity point
        //
        // NB: There is no subgroup check for the G2 addition precompile.
        //
        // We use blst_p2_affine_on_curve instead of blst_p2_affine_in_g2
        // because the latter performs the subgroup check.
        //
        // SAFETY: Out is a blst value.
        if unsafe { !blst_p2_affine_on_curve(&out) } {
            return Err("Element not on G2 curve".to_string());
        }
    }

    Ok(out)
}
