use cfx_types::{address_util::AddressUtil, AllChainID, Space, U256};
use cfx_vm_types::{ConsensusGasSpec, Spec};
use primitives::{
    block::BlockHeight,
    transaction::{
        native_transaction::TypedNativeTransaction, TransactionError,
    },
    Action, Transaction, TransactionWithSignature,
};

use crate::{
    executive::{eip7623_required_gas, gas_required_for},
    spec::TransitionsEpochHeight,
};

/// Local validation may either require a transaction to be packable now or
/// accept protocol types/epoch heights that can become valid later.
#[derive(Copy, Clone)]
pub enum LocalValidationMode {
    Full,
    MaybeLater,
}

/// Result of checking a transaction under the local packing rules.
#[derive(Copy, Clone)]
pub enum PackingCheckResult {
    /// Passes packing checks at the current height.
    Pack,
    /// Fails current packing checks but passes `MaybeLater` checks.
    Pending,
    /// Fails both current packing checks and `MaybeLater` checks.
    Drop,
}

/// Validation mode used by transaction admission and block synchronization.
#[derive(Copy, Clone)]
pub enum ValidationMode<'a> {
    Local(LocalValidationMode, &'a Spec),
    Remote(&'a ConsensusGasSpec),
}

impl<'a> ValidationMode<'a> {
    pub fn is_maybe_later(&self) -> bool {
        matches!(self, Self::Local(LocalValidationMode::MaybeLater, _))
    }
}

/// Inputs required by transaction-only protocol validation.
pub struct ValidationContext<'a> {
    pub chain_id: AllChainID,
    pub height: BlockHeight,
    pub transitions: &'a TransitionsEpochHeight,
    pub transaction_epoch_bound: u64,
    pub max_nonce: Option<U256>,
    pub mode: ValidationMode<'a>,
}

/// Validate transaction-only protocol rules shared by local admission,
/// execution revalidation, and sync block verification.
pub fn validate_transaction_common(
    tx: &TransactionWithSignature, context: &ValidationContext<'_>,
) -> Result<(), TransactionError> {
    let ValidationContext {
        chain_id,
        height,
        transitions,
        transaction_epoch_bound,
        max_nonce,
        mode,
    } = context;

    tx.check_low_s()?;
    tx.check_y_parity()?;

    if tx.is_unsigned() {
        return Err(TransactionError::InvalidSignature(
            "Transaction is unsigned".into(),
        ));
    }

    if let Some(tx_chain_id) = tx.chain_id() {
        if tx_chain_id != chain_id.in_space(tx.space()) {
            return Err(TransactionError::ChainIdMismatch {
                expected: chain_id.in_space(tx.space()),
                got: tx_chain_id,
                space: tx.space(),
            });
        }
    }

    if tx.gas_price().is_zero() {
        return Err(TransactionError::ZeroGasPrice);
    }

    if matches!(*mode, ValidationMode::Local(..)) && tx.space() == Space::Native
    {
        if let Action::Call(ref address) = tx.transaction.action() {
            if !address.is_genesis_valid_address() {
                return Err(TransactionError::InvalidReceiver);
            }
        }
    }

    if let (ValidationMode::Local(..), Some(max_nonce)) = (*mode, *max_nonce) {
        if tx.nonce() > &max_nonce {
            return Err(TransactionError::TooLargeNonce);
        }
    }

    let cip76 = *height >= transitions.cip76;
    let cip90a = *height >= transitions.cip90a;
    let cip130 =
        *height >= transitions.cip130 && *height < transitions.align_evm;
    let cip1559 = *height >= transitions.cip1559;
    let cip7702 = *height >= transitions.cip7702;
    let cip645 = *height >= transitions.cip645;
    let eip7623 = *height >= transitions.eip7623;
    let cip172 = *height >= transitions.cip172;

    if let Transaction::Native(ref native_tx) = tx.unsigned {
        verify_transaction_epoch_height(
            native_tx,
            *height,
            *transaction_epoch_bound,
            mode,
        )?;
    }

    if !check_eip155_transaction(tx, cip90a, mode) {
        return Err(TransactionError::FutureTransactionType {
            tx_type: primitives::transaction::LEGACY_TX_TYPE,
            current_height: *height,
            enable_height: transitions.cip90a,
        });
    }

    if !check_eip1559_transaction(tx, cip1559, mode) {
        return Err(TransactionError::FutureTransactionType {
            tx_type: primitives::transaction::EIP1559_TYPE,
            current_height: *height,
            enable_height: transitions.cip1559,
        });
    }

    if !check_eip7702_transaction(tx, cip7702, mode) {
        return Err(TransactionError::FutureTransactionType {
            tx_type: primitives::transaction::EIP7702_TYPE,
            current_height: *height,
            enable_height: transitions.cip7702,
        });
    }

    if !check_eip3860(tx, cip645) {
        return Err(TransactionError::CreateInitCodeSizeLimit);
    }

    check_eip1559_validation(tx, cip645)?;
    check_eip7702_validation(tx)?;
    check_gas_limit(tx, cip76, eip7623, mode)?;
    check_gas_limit_with_calldata(tx, cip130)?;
    check_canonical_rlp(tx, cip172, mode)
}

/// Check height-dependent rules before selecting a transaction for packing.
///
/// The caller remains responsible for validation that does not depend on the
/// current height.
pub fn check_transaction_for_packing(
    tx: &TransactionWithSignature, height: BlockHeight,
    transitions: &TransitionsEpochHeight, transaction_epoch_bound: u64,
    spec: &Spec,
) -> PackingCheckResult {
    let passes_packing_checks = |mode: &ValidationMode<'_>| {
        let cip90a = height >= transitions.cip90a;
        let cip1559 = height >= transitions.cip1559;
        let cip7702 = height >= transitions.cip7702;
        let cip645 = height >= transitions.cip645;

        if !check_eip1559_transaction(tx, cip1559, mode) {
            log::trace!(
                "check_transaction_for_packing: EIP-1559 check failed at height {} txhash={:?}",
                height,
                tx.hash()
            );
            return false;
        }

        if !check_eip7702_transaction(tx, cip7702, mode) {
            log::trace!(
                "check_transaction_for_packing: EIP-7702 check failed at height {} txhash={:?}",
                height,
                tx.hash()
            );
            return false;
        }

        if !check_eip3860(tx, cip645) {
            log::trace!(
                "check_transaction_for_packing: EIP-3860 check failed at height {} txhash={:?}",
                height,
                tx.hash()
            );
            return false;
        }

        if let Transaction::Native(ref native_tx) = tx.unsigned {
            verify_transaction_epoch_height(
                native_tx,
                height,
                transaction_epoch_bound,
                mode,
            )
            .is_ok()
        } else {
            check_eip155_transaction(tx, cip90a, mode)
        }
    };

    let packing_mode = ValidationMode::Local(LocalValidationMode::Full, spec);
    let pool_mode =
        ValidationMode::Local(LocalValidationMode::MaybeLater, spec);

    match (
        passes_packing_checks(&packing_mode),
        passes_packing_checks(&pool_mode),
    ) {
        (true, _) => PackingCheckResult::Pack,
        (false, true) => PackingCheckResult::Pending,
        (false, false) => PackingCheckResult::Drop,
    }
}

pub fn check_transaction_epoch_bound(
    tx: &TypedNativeTransaction, block_height: u64,
    transaction_epoch_bound: u64,
) -> i8 {
    if tx.epoch_height().wrapping_add(transaction_epoch_bound) < block_height {
        -1
    } else if *tx.epoch_height() > block_height + transaction_epoch_bound {
        1
    } else {
        0
    }
}

pub fn verify_transaction_epoch_height(
    tx: &TypedNativeTransaction, block_height: u64,
    transaction_epoch_bound: u64, mode: &ValidationMode,
) -> Result<(), TransactionError> {
    let result = check_transaction_epoch_bound(
        tx,
        block_height,
        transaction_epoch_bound,
    );
    if result == 0 || (result > 0 && mode.is_maybe_later()) {
        Ok(())
    } else {
        Err(TransactionError::EpochHeightOutOfBound {
            set: *tx.epoch_height(),
            block_height,
            transaction_epoch_bound,
        })
    }
}

pub fn check_eip155_transaction(
    tx: &TransactionWithSignature, cip90a: bool, mode: &ValidationMode,
) -> bool {
    if tx.space() == Space::Native {
        return true;
    }
    match mode {
        ValidationMode::Local(LocalValidationMode::Full, spec) => {
            cip90a && spec.cip90
        }
        ValidationMode::Local(LocalValidationMode::MaybeLater, _) => true,
        ValidationMode::Remote(_) => cip90a,
    }
}

pub fn check_eip1559_transaction(
    tx: &TransactionWithSignature, cip1559: bool, mode: &ValidationMode,
) -> bool {
    if tx.is_legacy() {
        return true;
    }
    match mode {
        ValidationMode::Local(LocalValidationMode::Full, spec) => {
            cip1559 && spec.cip1559
        }
        ValidationMode::Local(LocalValidationMode::MaybeLater, _) => true,
        ValidationMode::Remote(_) => cip1559,
    }
}

pub fn check_eip7702_transaction(
    tx: &TransactionWithSignature, cip7702: bool, mode: &ValidationMode,
) -> bool {
    if !tx.after_7702() {
        return true;
    }
    match mode {
        ValidationMode::Local(LocalValidationMode::Full, spec) => {
            cip7702 && spec.cip7702
        }
        ValidationMode::Local(LocalValidationMode::MaybeLater, _) => true,
        ValidationMode::Remote(_) => cip7702,
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

pub fn check_eip3860(tx: &TransactionWithSignature, cip645: bool) -> bool {
    const SPEC: Spec = Spec::genesis_spec();
    !cip645
        || tx.action() != Action::Create
        || tx.data().len() <= SPEC.init_code_data_limit
}

fn check_gas_limit(
    tx: &TransactionWithSignature, cip76: bool, eip7623: bool,
    mode: &ValidationMode,
) -> Result<(), TransactionError> {
    let consensus_spec = match mode {
        ValidationMode::Local(_, spec) => spec.to_consensus_spec(),
        ValidationMode::Remote(spec) => {
            if !eip7623 && cip76 {
                return Ok(());
            }
            (*spec).clone()
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
        return Err(TransactionError::NotEnoughBaseGas {
            required: tx_intrinsic_gas.into(),
            got: *tx.gas(),
        });
    }

    if eip7623 {
        let floor_gas = eip7623_required_gas(&tx.data(), &consensus_spec);
        if *tx.gas() < floor_gas.into() {
            return Err(TransactionError::NotEnoughBaseGas {
                required: floor_gas.into(),
                got: *tx.gas(),
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
    let min_gas_limit = tx.data().len().saturating_mul(100);
    if tx.gas() < &U256::from(min_gas_limit) {
        return Err(TransactionError::NotEnoughBaseGas {
            required: min_gas_limit.into(),
            got: *tx.gas(),
        });
    }
    Ok(())
}

fn check_canonical_rlp(
    tx: &TransactionWithSignature, cip172: bool, mode: &ValidationMode,
) -> Result<(), TransactionError> {
    if tx.is_canonical_rlp() {
        return Ok(());
    }
    let rejected = match mode {
        ValidationMode::Local(..) => true,
        ValidationMode::Remote(_) => cip172,
    };
    if rejected {
        return Err(TransactionError::InvalidRlp(
            "non-canonical transaction RLP encoding".into(),
        ));
    }
    Ok(())
}
