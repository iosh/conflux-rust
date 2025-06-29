// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

mod cache;
mod compute;
mod keccak;
mod seed_compute;
mod shared;
mod target_difficulty_manager;
mod traits;

pub use traits::ConsensusProvider;

pub use self::{
    cache::CacheBuilder, shared::POW_STAGE_LENGTH,
    target_difficulty_manager::TargetDifficultyManager,
};
use keccak_hash::keccak as keccak_hash;

use cfx_parameters::pow::*;
use cfx_types::{BigEndianHash, H256, U256, U512};

use malloc_size_of_derive::MallocSizeOf as DeriveMallocSizeOf;
use primitives::BlockHeader;
use static_assertions::_core::str::FromStr;
use std::convert::TryFrom;

#[cfg(target_endian = "big")]
compile_error!("The PoW implementation requires little-endian platform");

#[derive(Debug, Copy, Clone, PartialOrd, PartialEq)]
pub struct ProofOfWorkProblem {
    pub block_height: u64,
    pub block_hash: H256,
    pub difficulty: U256,
    pub boundary: U256,
}

impl ProofOfWorkProblem {
    pub const NO_BOUNDARY: U256 = U256::MAX;

    pub fn new(block_height: u64, block_hash: H256, difficulty: U256) -> Self {
        let boundary = difficulty_to_boundary(&difficulty);
        Self {
            block_height,
            block_hash,
            difficulty,
            boundary,
        }
    }

    pub fn from_block_header(block_header: &BlockHeader) -> Self {
        Self::new(
            block_header.height(),
            block_header.problem_hash(),
            *block_header.difficulty(),
        )
    }

    #[inline]
    pub fn validate_hash_against_boundary(
        hash: &H256, nonce: &U256, boundary: &U256,
    ) -> bool {
        let lower_bound = nonce_to_lower_bound(nonce);
        let (against_lower_bound_u256, _) =
            BigEndianHash::into_uint(hash).overflowing_sub(lower_bound);
        against_lower_bound_u256.lt(boundary)
            || boundary.eq(&ProofOfWorkProblem::NO_BOUNDARY)
    }
}

#[derive(Debug, Copy, Clone)]
pub struct ProofOfWorkSolution {
    pub nonce: U256,
}

#[derive(Debug, Clone, DeriveMallocSizeOf)]
pub enum MiningType {
    Stratum,
    CPU,
    Disable,
}

impl FromStr for MiningType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mining_type = match s {
            "stratum" => Self::Stratum,
            "cpu" => Self::CPU,
            "disable" => Self::Disable,
            _ => return Err("invalid mining type".into()),
        };
        Ok(mining_type)
    }
}

#[derive(Debug, Clone, DeriveMallocSizeOf)]
pub struct ProofOfWorkConfig {
    pub test_mode: bool,
    pub use_octopus_in_test_mode: bool,
    pub mining_type: MiningType,
    pub initial_difficulty: u64,
    pub block_generation_period: u64,
    pub stratum_listen_addr: String,
    pub stratum_port: u16,
    pub stratum_secret: Option<H256>,
    pub pow_problem_window_size: usize,
    pub cip86_height: u64,
}

impl ProofOfWorkConfig {
    pub fn new(
        test_mode: bool, use_octopus_in_test_mode: bool, mining_type: &str,
        initial_difficulty: Option<u64>, stratum_listen_addr: String,
        stratum_port: u16, stratum_secret: Option<H256>,
        pow_problem_window_size: usize, cip86_height: u64,
    ) -> Self {
        if test_mode {
            ProofOfWorkConfig {
                test_mode,
                use_octopus_in_test_mode,
                mining_type: mining_type.parse().expect("Invalid mining type"),
                initial_difficulty: initial_difficulty.unwrap_or(4),
                block_generation_period: 1000000,
                stratum_listen_addr,
                stratum_port,
                stratum_secret,
                pow_problem_window_size,
                cip86_height,
            }
        } else {
            ProofOfWorkConfig {
                test_mode,
                use_octopus_in_test_mode,
                mining_type: mining_type.parse().expect("Invalid mining type"),
                initial_difficulty: INITIAL_DIFFICULTY,
                block_generation_period: TARGET_AVERAGE_BLOCK_GENERATION_PERIOD,
                stratum_listen_addr,
                stratum_port,
                stratum_secret,
                pow_problem_window_size,
                cip86_height,
            }
        }
    }

    pub fn difficulty_adjustment_epoch_period(&self, cur_height: u64) -> u64 {
        if self.test_mode {
            20
        } else {
            if cur_height > self.cip86_height {
                DIFFICULTY_ADJUSTMENT_EPOCH_PERIOD_CIP
            } else {
                DIFFICULTY_ADJUSTMENT_EPOCH_PERIOD
            }
        }
    }

    pub fn use_octopus(&self) -> bool {
        !self.test_mode || self.use_octopus_in_test_mode
    }

    pub fn use_stratum(&self) -> bool {
        matches!(self.mining_type, MiningType::Stratum)
    }

    pub fn enable_mining(&self) -> bool {
        !matches!(self.mining_type, MiningType::Disable)
    }

    pub fn target_difficulty(
        &self, block_count: u64, timespan: u64, cur_difficulty: &U256,
    ) -> U256 {
        if timespan == 0 || block_count <= 1 || self.test_mode {
            return self.initial_difficulty.into();
        }

        let target = (U512::from(*cur_difficulty)
            * U512::from(self.block_generation_period)
            // - 1 for unbiased estimation, like stdvar
            * U512::from(block_count - 1))
            / (U512::from(timespan) * U512::from(1000000));
        if target.is_zero() {
            return 1.into();
        }
        if target > U256::max_value().into() {
            return U256::max_value();
        }
        U256::try_from(target).unwrap()
    }

    pub fn get_adjustment_bound(&self, diff: U256) -> (U256, U256) {
        let adjustment = diff / DIFFICULTY_ADJUSTMENT_FACTOR;
        let mut min_diff = diff - adjustment;
        let mut max_diff = diff + adjustment;
        let initial_diff: U256 = self.initial_difficulty.into();

        if min_diff < initial_diff {
            min_diff = initial_diff;
        }

        if max_diff < min_diff {
            max_diff = min_diff;
        }

        (min_diff, max_diff)
    }
}

// We will use the top 128 bits (excluding the highest bit) to be the lower
// bound of our PoW. The rationale is to provide a solution for block
// withholding attack among mining pools.
pub fn nonce_to_lower_bound(nonce: &U256) -> U256 {
    let mut buf = [0u8; 32];
    nonce.to_big_endian(&mut buf[..]);
    for i in 16..32 {
        buf[i] = 0;
    }
    buf[0] = buf[0] & 0x7f;
    // Note that U256::from assumes big_endian of the bytes
    let lower_bound = U256::from(buf);
    lower_bound
}

pub fn pow_hash_to_quality(hash: &H256, nonce: &U256) -> U256 {
    let hash_as_uint = BigEndianHash::into_uint(hash);
    let lower_bound = nonce_to_lower_bound(nonce);
    let (against_bound_u256, _) = hash_as_uint.overflowing_sub(lower_bound);
    if against_bound_u256.eq(&U256::MAX) {
        U256::one()
    } else {
        boundary_to_difficulty(&(against_bound_u256 + U256::one()))
    }
}

/// This should only be used in tests.
pub fn pow_quality_to_hash(pow_quality: &U256, nonce: &U256) -> H256 {
    let lower_bound = nonce_to_lower_bound(nonce);
    let hash_u256 = if pow_quality.eq(&U256::MAX) {
        U256::one()
    } else {
        let boundary = difficulty_to_boundary(&(pow_quality + U256::one()));
        let (against_bound_u256, _) = boundary.overflowing_add(lower_bound);
        against_bound_u256
    };
    BigEndianHash::from_uint(&hash_u256)
}

/// Convert boundary to its original difficulty. Basically just `f(x) = 2^256 /
/// x`.
pub fn boundary_to_difficulty(boundary: &U256) -> U256 {
    assert!(!boundary.is_zero());
    if boundary.eq(&U256::one()) {
        U256::MAX
    } else {
        compute_inv_x_times_2_pow_256_floor(boundary)
    }
}

/// Convert difficulty to the target boundary. Basically just `f(x) = 2^256 /
/// x`.
pub fn difficulty_to_boundary(difficulty: &U256) -> U256 {
    assert!(!difficulty.is_zero());
    if difficulty.eq(&U256::one()) {
        ProofOfWorkProblem::NO_BOUNDARY
    } else {
        compute_inv_x_times_2_pow_256_floor(difficulty)
    }
}

/// Compute [2^256 / x], where x >= 2 and x < 2^256.
pub fn compute_inv_x_times_2_pow_256_floor(x: &U256) -> U256 {
    let (div, modular) = U256::MAX.clone().div_mod(x.clone());
    if &(modular + U256::one()) == x {
        div + U256::one()
    } else {
        div
    }
}

pub struct PowComputer {
    use_octopus: bool,
    cache_builder: CacheBuilder,
}

impl PowComputer {
    pub fn new(use_octopus: bool) -> Self {
        PowComputer {
            use_octopus,
            cache_builder: CacheBuilder::new(),
        }
    }

    pub fn compute(
        &self, nonce: &U256, block_hash: &H256, block_height: u64,
    ) -> H256 {
        if !self.use_octopus {
            let mut buf = [0u8; 64];
            for i in 0..32 {
                buf[i] = block_hash[i];
            }
            nonce.to_little_endian(&mut buf[32..64]);
            let intermediate = keccak_hash(&buf[..]);
            let mut tmp = [0u8; 32];
            for i in 0..32 {
                tmp[i] = intermediate[i] ^ block_hash[i]
            }
            keccak_hash(tmp)
        } else {
            let light = self.cache_builder.light(block_height);
            light
                .compute(block_hash.as_fixed_bytes(), nonce.low_u64())
                .into()
        }
    }

    pub fn validate(
        &self, problem: &ProofOfWorkProblem, solution: &ProofOfWorkSolution,
    ) -> bool {
        let nonce = solution.nonce;
        let hash =
            self.compute(&nonce, &problem.block_hash, problem.block_height);
        ProofOfWorkProblem::validate_hash_against_boundary(
            &hash,
            &nonce,
            &problem.boundary,
        )
    }
}

#[test]
fn test_octopus() {
    let pow = PowComputer::new(true);

    let block_hash =
        "4d99d0b41c7eb0dd1a801c35aae2df28ae6b53bc7743f0818a34b6ec97f5b4ae"
            .parse()
            .unwrap();
    let start_nonce = 0x2333333333u64 & (!0x1f);
    pow.compute(&U256::from(start_nonce), &block_hash, 2);
}
