// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

use crate::{
    executive::{eip7623_required_gas, gas_required_for},
    spec::TransitionsEpochHeight,
};
use cfx_types::{address_util::AddressUtil, AllChainID, Space, U256};
use cfx_vm_types::{ConsensusGasSpec, Spec};
use primitives::{
    block::BlockHeight,
    transaction::{
        native_transaction::TypedNativeTransaction, TransactionError,
        EIP1559_TYPE, EIP7702_TYPE, LEGACY_TX_TYPE,
    },
    Action, Transaction, TransactionWithSignature,
};

#[derive(Debug, Clone)]
pub struct TransactionVerifier {
    pub transaction_epoch_bound: u64,
    pub max_nonce: Option<U256>,
}

impl TransactionVerifier {
    pub fn new(transaction_epoch_bound: u64, nonce_bits: usize) -> Self {
        Self {
            transaction_epoch_bound,
            max_nonce: if nonce_bits < 256 {
                Some((U256::one() << nonce_bits) - 1)
            } else {
                None
            },
        }
    }

    pub fn check_transaction_epoch_bound(
        tx: &TypedNativeTransaction, block_height: u64,
        transaction_epoch_bound: u64,
    ) -> i8 {
        if tx.epoch_height().wrapping_add(transaction_epoch_bound)
            < block_height
        {
            -1
        } else if *tx.epoch_height() > block_height + transaction_epoch_bound {
            1
        } else {
            0
        }
    }

    fn verify_transaction_epoch_height(
        tx: &TypedNativeTransaction, block_height: u64,
        transaction_epoch_bound: u64, mode: &VerifyTxMode,
    ) -> Result<(), TransactionError> {
        let result = Self::check_transaction_epoch_bound(
            tx,
            block_height,
            transaction_epoch_bound,
        );
        let allow_larger_epoch = mode.is_maybe_later();

        if result == 0 || (result > 0 && allow_larger_epoch) {
            Ok(())
        } else {
            bail!(TransactionError::EpochHeightOutOfBound {
                set: *tx.epoch_height(),
                block_height,
                transaction_epoch_bound,
            });
        }
    }

    fn fast_recheck_inner<F>(spec: &Spec, f: F) -> (bool, bool)
    where F: Fn(&VerifyTxMode) -> bool {
        let tx_pool_mode =
            VerifyTxMode::Local(VerifyTxLocalMode::MaybeLater, spec);
        let packing_mode = VerifyTxMode::Local(VerifyTxLocalMode::Full, spec);

        (f(&packing_mode), f(&tx_pool_mode))
    }

    pub fn fast_recheck(
        &self, tx: &TransactionWithSignature, height: BlockHeight,
        transitions: &TransitionsEpochHeight, spec: &Spec,
    ) -> PackingCheckResult {
        let cip90a = height >= transitions.cip90a;
        let cip1559 = height >= transitions.cip1559;
        let cip7702 = height >= transitions.cip7702;
        let cip645 = height >= transitions.cip645;

        let (can_pack, later_pack) = Self::fast_recheck_inner(
            spec,
            |mode: &VerifyTxMode| {
                if !Self::check_eip1559_transaction(tx, cip1559, mode) {
                    trace!(
                        "fast_recheck: EIP-1559 transaction check failed at height {} txhash={:?}",
                        height,
                        tx.hash()
                    );
                    return false;
                }

                if !Self::check_eip7702_transaction(tx, cip7702, mode) {
                    trace!(
                        "fast_recheck: EIP-7702 transaction check failed at height {} txhash={:?}",
                        height,
                        tx.hash()
                    );
                    return false;
                }

                if !Self::check_eip3860(tx, cip645) {
                    trace!(
                        "fast_recheck: EIP-3860 transaction check failed at height {} txhash={:?}",
                        height,
                        tx.hash()
                    );
                    return false;
                }

                if let Transaction::Native(ref tx) = tx.unsigned {
                    Self::verify_transaction_epoch_height(
                        tx,
                        height,
                        self.transaction_epoch_bound,
                        mode,
                    )
                    .is_ok()
                } else {
                    Self::check_eip155_transaction(tx, cip90a, mode)
                }
            },
        );

        match (can_pack, later_pack) {
            (true, _) => PackingCheckResult::Pack,
            (false, true) => PackingCheckResult::Pending,
            (false, false) => PackingCheckResult::Drop,
        }
    }

    // Packing transactions, verifying transaction in sync graph and inserting
    // transactions may have different logics. But they share a lot of similar
    // rules. We combine them together for convenient in the future upgrades..
    pub fn verify_transaction_common(
        &self, tx: &TransactionWithSignature, chain_id: AllChainID,
        height: BlockHeight, transitions: &TransitionsEpochHeight,
        mode: VerifyTxMode,
    ) -> Result<(), TransactionError> {
        tx.check_low_s()?;
        tx.check_y_parity()?;

        // Disallow unsigned transactions
        if tx.is_unsigned() {
            bail!(TransactionError::InvalidSignature(
                "Transaction is unsigned".into()
            ));
        }

        if let Some(tx_chain_id) = tx.chain_id() {
            if tx_chain_id != chain_id.in_space(tx.space()) {
                bail!(TransactionError::ChainIdMismatch {
                    expected: chain_id.in_space(tx.space()),
                    got: tx_chain_id,
                    space: tx.space(),
                });
            }
        }

        // Forbid zero-gas-price tx
        if tx.gas_price().is_zero() {
            bail!(TransactionError::ZeroGasPrice);
        }

        if matches!(mode, VerifyTxMode::Local(..))
            && tx.space() == Space::Native
        {
            if let Action::Call(ref address) = tx.transaction.action() {
                if !address.is_genesis_valid_address() {
                    bail!(TransactionError::InvalidReceiver)
                }
            }
        }

        if let (VerifyTxMode::Local(..), Some(max_nonce)) =
            (mode, self.max_nonce)
        {
            if tx.nonce() > &max_nonce {
                bail!(TransactionError::TooLargeNonce)
            }
        }

        // ******************************************
        // Each constraint depends on a mode or a CIP should be
        // implemented in a seperated function.
        // ******************************************
        let cip76 = height >= transitions.cip76;
        let cip90a = height >= transitions.cip90a;
        let cip130 =
            height >= transitions.cip130 && height < transitions.align_evm;
        let cip1559 = height >= transitions.cip1559;
        let cip7702 = height >= transitions.cip7702;
        let cip645 = height >= transitions.cip645;
        let eip7623 = height >= transitions.eip7623;
        let cip172 = height >= transitions.cip172;

        if let Transaction::Native(ref tx) = tx.unsigned {
            Self::verify_transaction_epoch_height(
                tx,
                height,
                self.transaction_epoch_bound,
                &mode,
            )?;
        }

        if !Self::check_eip155_transaction(tx, cip90a, &mode) {
            bail!(TransactionError::FutureTransactionType {
                tx_type: LEGACY_TX_TYPE,
                current_height: height,
                enable_height: transitions.cip90a,
            });
        }

        if !Self::check_eip1559_transaction(tx, cip1559, &mode) {
            bail!(TransactionError::FutureTransactionType {
                tx_type: EIP1559_TYPE,
                current_height: height,
                enable_height: transitions.cip1559,
            })
        }

        if !Self::check_eip7702_transaction(tx, cip7702, &mode) {
            bail!(TransactionError::FutureTransactionType {
                tx_type: EIP7702_TYPE,
                current_height: height,
                enable_height: transitions.cip7702,
            })
        }

        if !Self::check_eip3860(tx, cip645) {
            bail!(TransactionError::CreateInitCodeSizeLimit)
        }

        Self::check_eip1559_validation(tx, cip645)?;
        Self::check_eip7702_validation(tx)?;

        Self::check_gas_limit(tx, cip76, eip7623, &mode)?;
        Self::check_gas_limit_with_calldata(tx, cip130)?;
        Self::check_canonical_rlp(tx, cip172, &mode)?;

        Ok(())
    }

    /// A non-canonical encoding hashes to a value the canonical re-encoding
    /// won't reproduce, so packing one orphans the block. Local (mempool /
    /// packing / RPC) rejects it outright; as a block-validity rule this is a
    /// consensus change, so Remote only rejects at/after the gate height.
    fn check_canonical_rlp(
        tx: &TransactionWithSignature, cip172: bool, mode: &VerifyTxMode,
    ) -> Result<(), TransactionError> {
        if tx.is_canonical_rlp() {
            return Ok(());
        }
        let rejected = match mode {
            VerifyTxMode::Local(..) => true,
            VerifyTxMode::Remote(_) => cip172,
        };
        if rejected {
            bail!(TransactionError::InvalidRlp(
                "non-canonical transaction RLP encoding".into()
            ));
        }
        Ok(())
    }

    fn check_eip155_transaction(
        tx: &TransactionWithSignature, cip90a: bool, mode: &VerifyTxMode,
    ) -> bool {
        if tx.space() == Space::Native {
            return true;
        }

        use VerifyTxLocalMode::*;
        match mode {
            VerifyTxMode::Local(Full, spec) => cip90a && spec.cip90,
            VerifyTxMode::Local(MaybeLater, _spec) => true,
            VerifyTxMode::Remote(_) => cip90a,
        }
    }

    fn check_eip1559_transaction(
        tx: &TransactionWithSignature, cip1559: bool, mode: &VerifyTxMode,
    ) -> bool {
        if tx.is_legacy() {
            return true;
        }

        use VerifyTxLocalMode::*;
        match mode {
            VerifyTxMode::Local(Full, spec) => cip1559 && spec.cip1559,
            VerifyTxMode::Local(MaybeLater, _spec) => true,
            VerifyTxMode::Remote(_) => cip1559,
        }
    }

    fn check_eip7702_transaction(
        tx: &TransactionWithSignature, cip7702: bool, mode: &VerifyTxMode,
    ) -> bool {
        if !tx.after_7702() {
            return true;
        }

        use VerifyTxLocalMode::*;
        match mode {
            VerifyTxMode::Local(Full, spec) => cip7702 && spec.cip7702,
            VerifyTxMode::Local(MaybeLater, _spec) => true,
            VerifyTxMode::Remote(_) => cip7702,
        }
    }

    fn check_eip7702_validation(
        tx: &TransactionWithSignature,
    ) -> Result<(), TransactionError> {
        if let Some(author_list) = tx.authorization_list() {
            if author_list.is_empty() {
                return Err(TransactionError::EmptyAuthorizationList);
            }
        }
        Ok(())
    }

    fn check_eip1559_validation(
        tx: &TransactionWithSignature, cip645: bool,
    ) -> Result<(), TransactionError> {
        if !cip645 || !tx.after_1559() {
            return Ok(());
        }

        if tx.max_priority_gas_price() > tx.gas_price() {
            return Err(TransactionError::PriortyGreaterThanMaxFee);
        }
        Ok(())
    }

    fn check_eip3860(tx: &TransactionWithSignature, cip645: bool) -> bool {
        // TODO: better way to get the specification
        const SPEC: Spec = Spec::genesis_spec();
        if !cip645 {
            return true;
        }
        if tx.action() != Action::Create {
            return true;
        }

        tx.data().len() <= SPEC.init_code_data_limit
    }

    /// Check transaction intrinsic gas. Influenced by CIP-76 and EIP-7623.
    fn check_gas_limit(
        tx: &TransactionWithSignature, cip76: bool, eip7623: bool,
        mode: &VerifyTxMode,
    ) -> Result<(), TransactionError> {
        let consensus_spec = match mode {
            VerifyTxMode::Local(_, spec) => spec.to_consensus_spec(),
            VerifyTxMode::Remote(spec) => {
                if !eip7623 && cip76 {
                    return Ok(());
                } else {
                    (*spec).clone()
                }
            }
        };

        let tx_intrinsic_gas = gas_required_for(
            tx.action() == Action::Create,
            &tx.data(),
            tx.access_list(),
            tx.authorization_len(),
            &consensus_spec,
        );

        if *tx.gas() < tx_intrinsic_gas.into() {
            bail!(TransactionError::NotEnoughBaseGas {
                required: tx_intrinsic_gas.into(),
                got: *tx.gas()
            });
        }

        if eip7623 {
            let floor_gas = eip7623_required_gas(&tx.data(), &consensus_spec);

            if *tx.gas() < floor_gas.into() {
                bail!(TransactionError::NotEnoughBaseGas {
                    required: floor_gas.into(),
                    got: *tx.gas()
                });
            }
        }

        Ok(())
    }

    fn check_gas_limit_with_calldata(
        tx: &TransactionWithSignature, cip130: bool,
    ) -> Result<(), TransactionError> {
        if !cip130 {
            return Ok(());
        }
        let data_length = tx.data().len();
        let min_gas_limit = data_length.saturating_mul(100);
        if tx.gas() < &U256::from(min_gas_limit) {
            bail!(TransactionError::NotEnoughBaseGas {
                required: min_gas_limit.into(),
                got: *tx.gas()
            });
        }
        Ok(())
    }
}

#[derive(Copy, Clone)]
pub enum PackingCheckResult {
    Pack,
    // Transaction can be packed.
    Pending,
    // Transaction may be ready to packed in the future.
    Drop, // Transaction can never be packed.
}

#[derive(Copy, Clone)]
pub enum VerifyTxMode<'a> {
    /// Check transactions in local mode, may have more constraints
    Local(VerifyTxLocalMode, &'a Spec),
    /// Check transactions for received blocks in sync graph, may have less
    /// constraints
    Remote(&'a ConsensusGasSpec),
}

#[derive(Copy, Clone)]
pub enum VerifyTxLocalMode {
    /// Apply all checks
    Full,
    /// If a transaction is not valid now, but can become valid in the future,
    /// the check sould pass
    MaybeLater,
}

impl<'a> VerifyTxMode<'a> {
    fn is_maybe_later(&self) -> bool {
        if let VerifyTxMode::Local(VerifyTxLocalMode::MaybeLater, _) = self {
            true
        } else {
            false
        }
    }
}
