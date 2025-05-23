// This file is derived from revm (MIT licensed)
// Copyright (c) 2021-2025 draganrakita
// Modified by Conflux Foundation 2025

use crate::builtin::Pricer;

use super::{
    consts::{
        DISCOUNT_TABLE_G1_MSM, G1_INPUT_ITEM_LENGTH, G1_MSM_BASE_GAS_FEE,
        G1_MSM_INPUT_LENGTH, NBITS, SCALAR_LENGTH,
    },
    g1::{encode_g1_point, extract_g1_input},
    msm_gas::MsmPricer,
    utils::extract_scalar_input,
};
use blst::{
    blst_p1, blst_p1_affine, blst_p1_from_affine, blst_p1_to_affine, p1_affines,
};

/// Implements EIP-2537 G1MSM precompile.
/// G1 multi-scalar-multiplication call expects `160*k` bytes as an input that
/// is interpreted as byte concatenation of `k` slices each of them being a byte
/// concatenation of encoding of G1 point (`128` bytes) and encoding of a scalar
/// value (`32` bytes).
/// Output is an encoding of multi-scalar-multiplication operation result -
/// single G1 point (`128` bytes).
/// See also: <https://eips.ethereum.org/EIPS/eip-2537#abi-for-g1-multiexponentiation>
pub(super) fn g1_msm(input: &[u8]) -> Result<Vec<u8>, String> {
    let input_len = input.len();
    if input_len == 0 || input_len % G1_MSM_INPUT_LENGTH != 0 {
        return Err(format!(
            "G1MSM input length should be multiple of {}, was {}",
            G1_MSM_INPUT_LENGTH, input_len
        ));
    }

    let k = input_len / G1_MSM_INPUT_LENGTH;

    let mut g1_points: Vec<blst_p1> = Vec::with_capacity(k);
    let mut scalars: Vec<u8> = Vec::with_capacity(k * SCALAR_LENGTH);
    for i in 0..k {
        let slice = &input[i * G1_MSM_INPUT_LENGTH
            ..i * G1_MSM_INPUT_LENGTH + G1_INPUT_ITEM_LENGTH];

        // BLST batch API for p1_affines blows up when you pass it a point at
        // infinity, so we must filter points at infinity (and their
        // corresponding scalars) from the input.
        if slice.iter().all(|i| *i == 0) {
            continue;
        }

        // NB: Scalar multiplications, MSMs and pairings MUST perform a subgroup
        // check.
        //
        // So we set the subgroup_check flag to `true`
        let p0_aff = &extract_g1_input(slice, true)?;

        let mut p0 = blst_p1::default();
        // SAFETY: `p0` and `p0_aff` are blst values.
        unsafe { blst_p1_from_affine(&mut p0, p0_aff) };
        g1_points.push(p0);

        scalars.extend_from_slice(
            &extract_scalar_input(
                &input[i * G1_MSM_INPUT_LENGTH + G1_INPUT_ITEM_LENGTH
                    ..i * G1_MSM_INPUT_LENGTH
                        + G1_INPUT_ITEM_LENGTH
                        + SCALAR_LENGTH],
            )?
            .b,
        );
    }

    // Return infinity point if all points are infinity
    if g1_points.is_empty() {
        return Ok([0; 128].into());
    }

    let points = p1_affines::from(&g1_points);
    let multiexp = points.mult(&scalars, NBITS);

    let mut multiexp_aff = blst_p1_affine::default();
    // SAFETY: `multiexp_aff` and `multiexp` are blst values.
    unsafe { blst_p1_to_affine(&mut multiexp_aff, &multiexp) };

    let out = encode_g1_point(&multiexp_aff);
    Ok(out)
}

pub(super) fn g1_msm_gas() -> impl Pricer {
    MsmPricer::new(
        G1_MSM_INPUT_LENGTH,
        &DISCOUNT_TABLE_G1_MSM,
        G1_MSM_BASE_GAS_FEE,
    )
}
