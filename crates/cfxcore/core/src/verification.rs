// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

use crate::{
    core_error::{BlockError, CoreError as Error},
    pow::{
        self, nonce_has_non_zero_high_128_bits, nonce_to_lower_bound,
        PowComputer, ProofOfWorkProblem,
    },
    sync::Error as SyncError,
};
pub use cfx_executor::transaction_validation::{
    LocalValidationMode as VerifyTxLocalMode, PackingCheckResult,
    ValidationMode as VerifyTxMode,
};
use cfx_executor::{
    machine::Machine, spec::TransitionsEpochHeight,
    transaction_validation as transaction,
};
use cfx_parameters::{block::*, consensus_internal::ELASTICITY_MULTIPLIER};
use cfx_storage::{
    into_simple_mpt_key, make_simple_mpt, simple_mpt_merkle_root,
    simple_mpt_proof, SimpleMpt, TrieProof,
};
use cfx_types::{AllChainID, BigEndianHash, Space, SpaceMap, H256, U256};
use cfx_vm_types::Spec;
use primitives::{
    block::BlockHeight,
    block_header::compute_next_price_tuple,
    transaction::{
        native_transaction::TypedNativeTransaction, TransactionError,
    },
    Block, BlockHeader, BlockReceipts, MerkleHash, Receipt, SignedTransaction,
    TransactionWithSignature,
};
use rlp::Encodable;
use rlp_derive::{RlpDecodable, RlpEncodable};
use serde_derive::{Deserialize, Serialize};
use std::{collections::HashSet, convert::TryInto, sync::Arc};
use unexpected::{Mismatch, OutOfBounds};

#[derive(Clone)]
pub struct VerificationConfig {
    pub verify_timestamp: bool,
    pub referee_bound: usize,
    pub max_block_size_in_bytes: usize,
    pub transaction_epoch_bound: u64,
    pub max_nonce: Option<U256>,
    machine: Arc<Machine>,
    pos_enable_height: u64,
}

/// Create an MPT from the ordered list of block transactions.
/// Keys are transaction indices, values are transaction hashes.
fn transaction_trie(transactions: &Vec<Arc<SignedTransaction>>) -> SimpleMpt {
    make_simple_mpt(
        transactions
            .iter()
            .map(|tx| tx.hash.as_bytes().into())
            .collect(),
    )
}

/// Compute block transaction root.
/// This value is stored in the `transactions_root` header field.
pub fn compute_transaction_root(
    transactions: &Vec<Arc<SignedTransaction>>,
) -> MerkleHash {
    simple_mpt_merkle_root(&mut transaction_trie(transactions))
}

/// Compute a proof for the `tx_index_in_block`-th transaction in a block.
pub fn compute_transaction_proof(
    transactions: &Vec<Arc<SignedTransaction>>, tx_index_in_block: usize,
) -> TrieProof {
    simple_mpt_proof(
        &mut transaction_trie(transactions),
        &into_simple_mpt_key(tx_index_in_block, transactions.len()),
    )
}

/// Create an MPT from the ordered list of block receipts.
/// Keys are receipt indices, values are RLP serialized receipts.
fn block_receipts_trie(block_receipts: &Vec<Receipt>) -> SimpleMpt {
    make_simple_mpt(
        block_receipts
            .iter()
            .map(|receipt| receipt.rlp_bytes().to_vec().into_boxed_slice())
            .collect(),
    )
}

/// Compute block receipts root.
fn compute_block_receipts_root(block_receipts: &Vec<Receipt>) -> MerkleHash {
    simple_mpt_merkle_root(&mut block_receipts_trie(block_receipts))
}

/// Compute a proof for the `tx_index_in_block`-th receipt in a block.
pub fn compute_block_receipt_proof(
    block_receipts: &Vec<Receipt>, tx_index_in_block: usize,
) -> TrieProof {
    simple_mpt_proof(
        &mut block_receipts_trie(block_receipts),
        &into_simple_mpt_key(tx_index_in_block, block_receipts.len()),
    )
}

/// Create an MPT from the ordered list of epoch receipts.
/// Keys are block indices in the epoch, values are block receipts roots.
fn epoch_receipts_trie(epoch_receipts: &Vec<Arc<BlockReceipts>>) -> SimpleMpt {
    make_simple_mpt(
        epoch_receipts
            .iter()
            .map(|block_receipts| &block_receipts.receipts)
            .map(|rs| compute_block_receipts_root(&rs).as_bytes().into())
            .collect(),
    )
}

/// Compute epoch receipts root.
/// This value is stored in the `deferred_receipts_root` header field.
pub fn compute_receipts_root(
    epoch_receipts: &Vec<Arc<BlockReceipts>>,
) -> MerkleHash {
    simple_mpt_merkle_root(&mut epoch_receipts_trie(epoch_receipts))
}

#[derive(
    Clone,
    Debug,
    RlpEncodable,
    RlpDecodable,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub struct EpochReceiptProof {
    pub block_index_proof: TrieProof,
    pub block_receipt_proof: TrieProof,
}

/// Compute a proof for the `tx_index_in_block`-th receipt
/// in the `block_index_in_epoch`-th block in an epoch.
pub fn compute_epoch_receipt_proof(
    epoch_receipts: &Vec<Arc<BlockReceipts>>, block_index_in_epoch: usize,
    tx_index_in_block: usize,
) -> EpochReceiptProof {
    let block_receipt_proof = compute_block_receipt_proof(
        &epoch_receipts[block_index_in_epoch].receipts,
        tx_index_in_block,
    );

    let block_index_proof = simple_mpt_proof(
        &mut epoch_receipts_trie(epoch_receipts),
        &into_simple_mpt_key(block_index_in_epoch, epoch_receipts.len()),
    );

    EpochReceiptProof {
        block_index_proof,
        block_receipt_proof,
    }
}

/// Use `proof` to verify that `tx_hash` is indeed the `tx_index_in_block`-th
/// transaction in a block with `num_txs_in_block` transactions and transaction
/// root `block_tx_root`.
pub fn is_valid_tx_inclusion_proof(
    block_tx_root: MerkleHash, tx_index_in_block: usize,
    num_txs_in_block: usize, tx_hash: H256, proof: &TrieProof,
) -> bool {
    let key = &into_simple_mpt_key(tx_index_in_block, num_txs_in_block);
    proof.is_valid_kv(key, Some(tx_hash.as_bytes()), &block_tx_root)
}

/// Use `block_index_proof` to get the correct block receipts trie root for the
/// `block_index_in_epoch`-th block in an epoch with `num_blocks_in_epoch`
/// blocks and receipts root `verified_epoch_receipts_root`.
/// Then, use `block_receipt_proof` to verify that `receipt` is indeed the
/// `tx_index_in_block`-th receipt in a block with `num_txs_in_block`
/// transactions and the transaction root from the previous step.
pub fn is_valid_receipt_inclusion_proof(
    verified_epoch_receipts_root: MerkleHash, block_index_in_epoch: usize,
    num_blocks_in_epoch: usize, block_index_proof: &TrieProof,
    tx_index_in_block: usize, num_txs_in_block: usize, receipt: &Receipt,
    block_receipt_proof: &TrieProof,
) -> bool {
    // get block receipts root from block index trie (proof)
    // traversing along `key` also means we're validating the proof
    let key = &into_simple_mpt_key(block_index_in_epoch, num_blocks_in_epoch);

    let block_receipts_root_bytes =
        match block_index_proof.get_value(key, &verified_epoch_receipts_root) {
            (false, _) => return false,
            (true, None) => return false,
            (true, Some(val)) => val,
        };

    // parse block receipts root as H256
    let block_receipts_root: H256 = match TryInto::<[u8; 32]>::try_into(
        block_receipts_root_bytes,
    ) {
        Ok(hash) => hash.into(),
        Err(e) => {
            // this should not happen
            error!(
                "Invalid content found in valid MPT: key = {:?}, value = {:?}; error = {:?}",
                key, block_receipts_root_bytes, e,
            );
            return false;
        }
    };

    // validate receipt in the block receipts trie
    let key = &into_simple_mpt_key(tx_index_in_block, num_txs_in_block);

    block_receipt_proof.is_valid_kv(
        key,
        Some(&receipt.rlp_bytes()[..]),
        &block_receipts_root,
    )
}

impl VerificationConfig {
    pub fn new(
        test_mode: bool, referee_bound: usize, max_block_size_in_bytes: usize,
        transaction_epoch_bound: u64, tx_pool_nonce_bits: usize,
        pos_enable_height: u64, machine: Arc<Machine>,
    ) -> Self {
        let max_nonce = if tx_pool_nonce_bits < 256 {
            Some((U256::one() << tx_pool_nonce_bits) - 1)
        } else {
            None
        };
        VerificationConfig {
            verify_timestamp: !test_mode,
            referee_bound,
            max_block_size_in_bytes,
            transaction_epoch_bound,
            machine,
            pos_enable_height,
            max_nonce,
        }
    }

    #[inline]
    /// Note that this function returns *pow_hash* of the block, not its quality
    pub fn get_or_fill_header_pow_hash(
        pow: &PowComputer, header: &mut BlockHeader,
    ) -> H256 {
        if header.pow_hash.is_none() {
            header.pow_hash = Some(Self::compute_pow_hash(pow, header));
        }
        header.pow_hash.unwrap()
    }

    pub fn get_or_fill_header_pow_quality(
        pow: &PowComputer, header: &mut BlockHeader,
    ) -> U256 {
        let pow_hash = Self::get_or_fill_header_pow_hash(pow, header);
        pow::pow_hash_to_quality(&pow_hash, &header.nonce())
    }

    pub fn get_or_compute_header_pow_quality(
        pow: &PowComputer, header: &BlockHeader,
    ) -> U256 {
        let pow_hash = header
            .pow_hash
            .unwrap_or_else(|| Self::compute_pow_hash(pow, header));
        pow::pow_hash_to_quality(&pow_hash, &header.nonce())
    }

    fn compute_pow_hash(pow: &PowComputer, header: &BlockHeader) -> H256 {
        let nonce = header.nonce();
        pow.compute(&nonce, &header.problem_hash(), header.height())
    }

    #[inline]
    pub fn verify_pow(
        &self, pow: &PowComputer, header: &mut BlockHeader,
    ) -> Result<(), Error> {
        if header.height()
            >= self.machine.params().transition_heights.cip_hn_fix
        {
            if nonce_has_non_zero_high_128_bits(&header.nonce()) {
                return Err(From::from(BlockError::InvalidNonce));
            }
        }
        let pow_hash = Self::get_or_fill_header_pow_hash(pow, header);
        if header.difficulty().is_zero() {
            return Err(BlockError::InvalidDifficulty(OutOfBounds {
                min: Some(0.into()),
                max: Some(0.into()),
                found: 0.into(),
            })
            .into());
        }
        let boundary = pow::difficulty_to_boundary(header.difficulty());
        if !ProofOfWorkProblem::validate_hash_against_boundary(
            &pow_hash,
            &header.nonce(),
            &boundary,
        ) {
            let lower_bound = nonce_to_lower_bound(&header.nonce());
            // Because the lower_bound first bit is always zero, as long as the
            // difficulty is not 1, this should not overflow.
            // We just use overflowing_add() here to be safe.
            let (upper_bound, _) = lower_bound.overflowing_add(boundary);
            warn!("block {} has invalid proof of work. boundary: [{}, {}), pow_hash: {}",
                  header.hash(), lower_bound.clone(), upper_bound.clone(), pow_hash.clone());
            return Err(From::from(BlockError::InvalidProofOfWork(
                OutOfBounds {
                    min: Some(BigEndianHash::from_uint(&lower_bound)),
                    max: Some(BigEndianHash::from_uint(&upper_bound)),
                    found: pow_hash,
                },
            )));
        }

        assert!(
            Self::get_or_fill_header_pow_quality(pow, header)
                >= *header.difficulty()
        );

        Ok(())
    }

    #[inline]
    pub fn validate_header_timestamp(
        &self, header: &BlockHeader, now: u64,
    ) -> Result<(), SyncError> {
        let invalid_threshold = now + VALID_TIME_DRIFT;
        if header.timestamp() > invalid_threshold {
            warn!("block {} has incorrect timestamp", header.hash());
            return Err(SyncError::InvalidTimestamp.into());
        }
        Ok(())
    }

    fn is_pos_enabled_at_height(&self, height: u64) -> bool {
        height >= self.pos_enable_height
    }

    /// Check basic header parameters.
    /// This does not require header to be graph or parental tree ready.
    #[inline]
    pub fn verify_header_params(
        &self, pow: &PowComputer, header: &mut BlockHeader,
    ) -> Result<(), Error> {
        // Check header custom data length
        let custom_len = header.custom_data_len();
        if custom_len > HEADER_CUSTOM_LENGTH_BOUND {
            return Err(From::from(BlockError::TooLongCustomInHeader(
                OutOfBounds {
                    min: Some(0),
                    max: Some(HEADER_CUSTOM_LENGTH_BOUND),
                    found: custom_len,
                },
            )));
        }

        if self.is_pos_enabled_at_height(header.height()) {
            if header.pos_reference().is_none() {
                bail!(BlockError::MissingPosReference);
            }
        } else {
            if header.pos_reference().is_some() {
                bail!(BlockError::UnexpectedPosReference);
            }
        }

        if header.height() >= self.machine.params().transition_heights.cip1559 {
            if header.base_price().is_none() {
                bail!(BlockError::MissingBaseFee);
            }
        } else {
            if header.base_price().is_some() {
                bail!(BlockError::UnexpectedBaseFee);
            }
        }

        // Note that this is just used to rule out deprecated blocks, so the
        // change of header struct actually happens before the change of
        // reward is reflected in the state root. The first state root
        // including results of new rewards will in the header after another
        // REWARD_EPOCH_COUNT + DEFERRED_STATE_EPOCH_COUNT epochs.
        if let Some(expected_custom_prefix) =
            self.machine.params().custom_prefix(header.height())
        {
            for (i, expected_bytes) in expected_custom_prefix.iter().enumerate()
            {
                // `None` => header custom too short; else prefix mismatch.
                let matches =
                    header.custom_item(i).is_some_and(|b| &b == expected_bytes);
                if !matches {
                    // Bound the error to the compared prefix; the header may
                    // carry a huge number of items.
                    let header_prefix = (0..expected_custom_prefix.len())
                        .filter_map(|j| header.custom_item(j))
                        .collect();
                    return Err(BlockError::InvalidCustom(
                        header_prefix,
                        expected_custom_prefix.clone(),
                    )
                    .into());
                }
            }
        }

        // verify POW
        self.verify_pow(pow, header)?;

        // A block will be invalid if it has more than REFEREE_BOUND referees
        if header.referee_hashes().len() > self.referee_bound {
            return Err(From::from(BlockError::TooManyReferees(OutOfBounds {
                min: Some(0),
                max: Some(self.referee_bound),
                found: header.referee_hashes().len(),
            })));
        }

        // verify non-duplicated parent and referee hashes
        let mut direct_ancestor_hashes = HashSet::new();
        let parent_hash = header.parent_hash();
        direct_ancestor_hashes.insert(parent_hash.clone());
        for referee_hash in header.referee_hashes() {
            if direct_ancestor_hashes.contains(referee_hash) {
                warn!(
                    "block {} has duplicate parent or referee hashes",
                    header.hash()
                );
                return Err(From::from(
                    BlockError::DuplicateParentOrRefereeHashes(
                        referee_hash.clone(),
                    ),
                ));
            }
            direct_ancestor_hashes.insert(referee_hash.clone());
        }

        Ok(())
    }

    /// Verify block data against header: transactions root
    #[inline]
    fn verify_block_integrity(&self, block: &Block) -> Result<(), Error> {
        let expected_root = compute_transaction_root(&block.transactions);
        if &expected_root != block.block_header.transactions_root() {
            warn!("Invalid transaction root");
            bail!(BlockError::InvalidTransactionsRoot(Mismatch {
                expected: expected_root,
                found: *block.block_header.transactions_root(),
            }));
        }
        Ok(())
    }

    /// Phase 1 quick block verification. Only does checks that are cheap.
    /// Operates on a single block.
    /// Note that we should first check whether the block body matches its
    /// header, e.g., check transaction root correctness, and then check the
    /// body itself. If body does not match header, the block may still be
    /// valid but we get the wrong body from evil node. So we try to get
    /// body again from others. However, if the body matches the header and
    /// the body is incorrect, this means the block is invalid, and we
    /// should discard this block and all its descendants.
    #[inline]
    pub fn verify_sync_graph_block_basic(
        &self, block: &Block, chain_id: AllChainID,
    ) -> Result<(), Error> {
        self.verify_block_integrity(block)?;

        let block_height = block.block_header.height();

        let mut block_size = 0;
        let transitions = &self.machine.params().transition_heights;
        let consensus_spec =
            &self.machine.params().consensus_spec(block_height);

        for t in &block.transactions {
            self.verify_transaction_common(
                t,
                chain_id,
                block_height,
                transitions,
                VerifyTxMode::Remote(consensus_spec),
            )?;
            block_size += t.rlp_size();
        }

        if block_size > self.max_block_size_in_bytes {
            return Err(From::from(BlockError::InvalidBlockSize(
                OutOfBounds {
                    min: None,
                    max: Some(self.max_block_size_in_bytes as u64),
                    found: block_size as u64,
                },
            )));
        }
        Ok(())
    }

    pub fn verify_sync_graph_ready_block(
        &self, block: &Block, parent: &BlockHeader,
    ) -> Result<(), Error> {
        let mut total_gas: SpaceMap<U256> = SpaceMap::default();
        for t in &block.transactions {
            // gas_limit is unbounded on this path, so the per-space sum can
            // exceed U256; a bare `+=` would panic instead of rejecting.
            let acc = &mut total_gas[t.space()];
            *acc = acc.checked_add(*t.gas_limit()).ok_or_else(|| {
                BlockError::InvalidPackedGasLimit(OutOfBounds {
                    min: None,
                    max: Some(*block.block_header.gas_limit()),
                    found: U256::MAX,
                })
            })?;
        }

        if block.block_header.height()
            >= self.machine.params().transition_heights.cip1559
        {
            self.check_base_fee(block, parent, total_gas)?;
        } else {
            self.check_hard_gas_limit(block, total_gas)?;
        }
        Ok(())
    }

    fn check_hard_gas_limit(
        &self, block: &Block, total_gas: SpaceMap<U256>,
    ) -> Result<(), Error> {
        let block_height = block.block_header.height();

        let evm_space_gas_limit =
            if self.machine.params().can_pack_evm_transaction(block_height) {
                *block.block_header.gas_limit()
                    / self.machine.params().evm_transaction_gas_ratio
            } else {
                U256::zero()
            };

        let evm_total_gas = total_gas[Space::Ethereum];
        // native + evm can exceed U256 even when each fits; avoid a
        // panicking sum.
        let block_total_gas = total_gas[Space::Native]
            .checked_add(total_gas[Space::Ethereum])
            .ok_or_else(|| {
                BlockError::InvalidPackedGasLimit(OutOfBounds {
                    min: None,
                    max: Some(*block.block_header.gas_limit()),
                    found: U256::MAX,
                })
            })?;

        if evm_total_gas > evm_space_gas_limit {
            return Err(From::from(BlockError::InvalidPackedGasLimit(
                OutOfBounds {
                    min: None,
                    max: Some(evm_space_gas_limit),
                    found: evm_total_gas,
                },
            )));
        }

        if block_total_gas > *block.block_header.gas_limit() {
            return Err(From::from(BlockError::InvalidPackedGasLimit(
                OutOfBounds {
                    min: None,
                    max: Some(*block.block_header.gas_limit()),
                    found: block_total_gas,
                },
            )));
        }

        Ok(())
    }

    fn check_base_fee(
        &self, block: &Block, parent: &BlockHeader, total_gas: SpaceMap<U256>,
    ) -> Result<(), Error> {
        use Space::*;

        let params = self.machine.params();
        let cip1559_init = params.transition_heights.cip1559;
        let block_height = block.block_header.height();

        assert!(block_height >= cip1559_init);

        let core_gas_limit = block.block_header.core_space_gas_limit();
        let espace_gas_limit = block
            .block_header
            .espace_gas_limit(params.can_pack_evm_transaction(block_height));

        if total_gas[Ethereum] > espace_gas_limit {
            return Err(From::from(BlockError::InvalidPackedGasLimit(
                OutOfBounds {
                    min: None,
                    max: Some(espace_gas_limit),
                    found: total_gas[Ethereum],
                },
            )));
        }

        if total_gas[Native] > core_gas_limit {
            return Err(From::from(BlockError::InvalidPackedGasLimit(
                OutOfBounds {
                    min: None,
                    max: Some(core_gas_limit),
                    found: total_gas[Native],
                },
            )));
        }

        let parent_base_price = if block_height == cip1559_init {
            params.init_base_price()
        } else {
            parent.base_price().unwrap()
        };

        let gas_limit = SpaceMap::new(core_gas_limit, espace_gas_limit);
        let gas_target = gas_limit.map_all(|x| x / ELASTICITY_MULTIPLIER);
        let min_base_price = params.min_base_price();

        let expected_base_price = SpaceMap::zip4(
            gas_target,
            total_gas,
            parent_base_price,
            min_base_price,
        )
        .map_all(compute_next_price_tuple);

        let actual_base_price = block.block_header.base_price().unwrap();

        if actual_base_price != expected_base_price {
            return Err(From::from(BlockError::InvalidBasePrice(Mismatch {
                expected: expected_base_price,
                found: actual_base_price,
            })));
        }

        Ok(())
    }

    pub fn check_transaction_epoch_bound(
        tx: &TypedNativeTransaction, block_height: u64,
        transaction_epoch_bound: u64,
    ) -> i8 {
        transaction::check_transaction_epoch_bound(
            tx,
            block_height,
            transaction_epoch_bound,
        )
    }

    pub fn fast_recheck(
        &self, tx: &TransactionWithSignature, height: BlockHeight,
        transitions: &TransitionsEpochHeight, spec: &Spec,
    ) -> PackingCheckResult {
        transaction::check_transaction_for_packing(
            tx,
            height,
            transitions,
            self.transaction_epoch_bound,
            spec,
        )
    }

    // Packing transactions, verifying transaction in sync graph and inserting
    // transactions may have different logics. But they share a lot of similar
    // rules. We combine them together for convenient in the future upgrades..
    pub fn verify_transaction_common(
        &self, tx: &TransactionWithSignature, chain_id: AllChainID,
        height: BlockHeight, transitions: &TransitionsEpochHeight,
        mode: VerifyTxMode,
    ) -> Result<(), TransactionError> {
        let context = transaction::ValidationContext {
            chain_id,
            height,
            transitions,
            transaction_epoch_bound: self.transaction_epoch_bound,
            max_nonce: self.max_nonce,
            mode,
        };
        transaction::validate_transaction_common(tx, &context)
    }

    pub fn check_tx_size(
        &self, tx: &TransactionWithSignature,
    ) -> Result<(), TransactionError> {
        if tx.rlp_size() > self.max_block_size_in_bytes {
            bail!(TransactionError::TooBig)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::verification::EpochReceiptProof;
    use cfx_storage::{
        CompressedPathRaw, TrieProof, TrieProofNode, VanillaChildrenTable,
    };

    #[test]
    fn test_rlp_epoch_receipt_proof() {
        let proof = EpochReceiptProof::default();
        assert_eq!(proof, rlp::decode(&rlp::encode(&proof)).unwrap());

        let serialized = serde_json::to_string(&proof).unwrap();
        let deserialized: EpochReceiptProof =
            serde_json::from_str(&serialized).unwrap();
        assert_eq!(proof, deserialized);

        let node1 = TrieProofNode::new(
            Default::default(),
            Some(Box::new([0x03, 0x04, 0x05])),
            CompressedPathRaw::new(
                &[0x00, 0x01, 0x02],
                CompressedPathRaw::first_nibble_mask(),
            ),
            /* path_without_first_nibble = */ true,
        );

        let root_node = {
            let mut children_table = VanillaChildrenTable::default();
            unsafe {
                *children_table.get_child_mut_unchecked(2) =
                    *node1.get_merkle();
                *children_table.get_children_count_mut() = 1;
            }
            TrieProofNode::new(
                children_table,
                None,
                CompressedPathRaw::default(),
                /* path_without_first_nibble = */ false,
            )
        };
        let nodes = [root_node, node1]
            .iter()
            .cloned()
            .cycle()
            .take(20)
            .collect();
        let proof = TrieProof::new(nodes).unwrap();

        let epoch_proof = EpochReceiptProof {
            block_index_proof: proof.clone(),
            block_receipt_proof: proof,
        };

        assert_eq!(
            epoch_proof,
            rlp::decode(&rlp::encode(&epoch_proof)).unwrap()
        );

        let serialized = serde_json::to_string(&epoch_proof).unwrap();
        let deserialized: EpochReceiptProof =
            serde_json::from_str(&serialized).unwrap();
        assert_eq!(epoch_proof, deserialized);
    }

    // A miner can pack transactions whose gas_limit values sum past U256; the
    // summation used to panic instead of rejecting the block.
    #[test]
    fn packed_gas_sum_overflow_is_rejected() {
        use crate::{
            core_error::{BlockError, CoreError as Error},
            verification::{compute_transaction_root, VerificationConfig},
        };
        use cfx_executor::{
            machine::{Machine, VmFactory},
            spec::CommonParams,
        };
        use cfx_types::U256;
        use cfxkey::{Generator, Random};
        use primitives::{
            transaction::native_transaction::{
                NativeTransaction, TypedNativeTransaction,
            },
            Action, Block, BlockHeaderBuilder, Transaction,
        };
        use std::sync::Arc;

        let params = CommonParams::default();
        let chain_id = params.chain_id.read().get_chain_id(1);
        let machine =
            Arc::new(Machine::new_with_builtin(params, VmFactory::new(1024)));
        let config = VerificationConfig::new(
            false,
            200,
            200 * 1024,
            100_000,
            128,
            u64::MAX,
            machine,
        );

        let keypair = Random.generate().unwrap();
        let tx = |nonce: u64| {
            Arc::new(
                Transaction::Native(TypedNativeTransaction::Cip155(
                    NativeTransaction {
                        nonce: nonce.into(),
                        gas_price: U256::one(),
                        // 2^255; two of these sum to 2^256.
                        gas: U256::one() << 255,
                        action: Action::Create,
                        value: U256::zero(),
                        storage_limit: 0,
                        epoch_height: 1,
                        chain_id: chain_id.in_native_space(),
                        data: vec![],
                    },
                ))
                .sign(keypair.secret()),
            )
        };
        let txs = vec![tx(0), tx(1)];

        let parent = BlockHeaderBuilder::new().with_height(0).build();
        let header = BlockHeaderBuilder::new()
            .with_height(1)
            .with_parent_hash(parent.hash())
            .with_transactions_root(compute_transaction_root(&txs))
            .with_gas_limit(30_000_000.into())
            .build();
        let block = Block::new(header, txs);

        assert!(matches!(
            config.verify_sync_graph_ready_block(&block, &parent),
            Err(Error::Block(BlockError::InvalidPackedGasLimit(_))),
        ));
    }
}
