// Copyright (c) The Diem Core Contributors
// SPDX-License-Identifier: Apache-2.0

// Copyright 2021 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

use anyhow::{format_err, Result};
use diem_crypto::{hash::SPARSE_MERKLE_PLACEHOLDER_HASH, HashValue};
use diem_types::{
    access_path::AccessPath,
    account_address::AccountAddress,
    account_state::AccountState,
    account_state_blob::{AccountStateBlob, AccountStateWithProof},
    committed_block::CommittedBlock,
    contract_event::ContractEvent,
    epoch_change::EpochChangeProof,
    epoch_state::EpochState,
    ledger_info::{
        deserialize_ledger_info_unchecked, LedgerInfoWithSignatures,
    },
    move_resource::MoveStorage,
    proof::{
        definition::LeafCount, AccumulatorConsistencyProof, SparseMerkleProof,
    },
    reward_distribution_event::RewardDistributionEventV2,
    term_state::PosState,
    transaction::{
        TransactionInfo, TransactionListWithProof, TransactionToCommit,
        TransactionWithProof, Version,
    },
};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    convert::TryFrom,
    sync::Arc,
};
use thiserror::Error;

// #[cfg(any(feature = "testing", feature = "fuzzing"))]
pub mod mock;
pub mod state_view;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StartupInfo {
    /// The latest ledger info.
    /// This struct is only used locally, so loaded signatures must be valid.
    #[serde(deserialize_with = "deserialize_ledger_info_unchecked")]
    pub latest_ledger_info: LedgerInfoWithSignatures,
    /// If the above ledger info doesn't carry a validator set, the latest
    /// validator set. Otherwise `None`.
    pub latest_epoch_state: Option<EpochState>,
    pub committed_tree_state: TreeState,
    pub synced_tree_state: Option<TreeState>,

    pub committed_pos_state: PosState,
}

impl StartupInfo {
    pub fn new(
        latest_ledger_info: LedgerInfoWithSignatures,
        latest_epoch_state: Option<EpochState>,
        committed_tree_state: TreeState, synced_tree_state: Option<TreeState>,
        committed_pos_state: PosState,
    ) -> Self {
        Self {
            latest_ledger_info,
            latest_epoch_state,
            committed_tree_state,
            synced_tree_state,
            committed_pos_state,
        }
    }

    #[cfg(any(feature = "fuzzing"))]
    pub fn new_for_testing() -> Self {
        use diem_types::on_chain_config::ValidatorSet;

        let latest_ledger_info = LedgerInfoWithSignatures::genesis(
            HashValue::zero(),
            ValidatorSet::empty(),
        );
        let latest_epoch_state = None;
        let committed_tree_state = TreeState {
            num_transactions: 0,
            ledger_frozen_subtree_hashes: Vec::new(),
            account_state_root_hash: *SPARSE_MERKLE_PLACEHOLDER_HASH,
        };
        let synced_tree_state = None;

        Self {
            latest_ledger_info,
            latest_epoch_state,
            committed_tree_state,
            synced_tree_state,
        }
    }

    pub fn get_epoch_state(&self) -> &EpochState {
        self.latest_ledger_info
            .ledger_info()
            .next_epoch_state()
            .unwrap_or_else(|| {
                self.latest_epoch_state
                    .as_ref()
                    .expect("EpochState must exist")
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TreeState {
    pub num_transactions: LeafCount,
    pub ledger_frozen_subtree_hashes: Vec<HashValue>,
    pub account_state_root_hash: HashValue,
}

impl TreeState {
    pub fn new(
        num_transactions: LeafCount,
        ledger_frozen_subtree_hashes: Vec<HashValue>,
        account_state_root_hash: HashValue,
    ) -> Self {
        Self {
            num_transactions,
            ledger_frozen_subtree_hashes,
            account_state_root_hash,
        }
    }

    pub fn describe(&self) -> &'static str {
        if self.num_transactions != 0 {
            "DB has been bootstrapped."
        } else if self.account_state_root_hash
            != *SPARSE_MERKLE_PLACEHOLDER_HASH
        {
            "DB has no transaction, but a non-empty pre-genesis state."
        } else {
            "DB is empty, has no transaction or state."
        }
    }
}

#[derive(Debug, Deserialize, Error, PartialEq, Serialize)]
pub enum Error {
    #[error("Service error: {:?}", error)]
    ServiceError { error: String },

    #[error("Serialization error: {0}")]
    SerializationError(String),
}

impl From<anyhow::Error> for Error {
    fn from(error: anyhow::Error) -> Self {
        Self::ServiceError {
            error: format!("{}", error),
        }
    }
}

impl From<bcs::Error> for Error {
    fn from(error: bcs::Error) -> Self {
        Self::SerializationError(format!("{}", error))
    }
}

impl From<diem_secure_net::Error> for Error {
    fn from(error: diem_secure_net::Error) -> Self {
        Self::ServiceError {
            error: format!("{}", error),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Order {
    Ascending,
    Descending,
}

/// Trait that is implemented by a DB that supports certain public (to client)
/// read APIs expected of a Diem DB
pub trait DbReader: Send + Sync {
    /// See [`DiemDB::get_epoch_ending_ledger_infos`].
    ///
    /// [`DiemDB::get_epoch_ending_ledger_infos`]:
    /// ../pos-ledger-db/struct.DiemDB.html#method.get_epoch_ending_ledger_infos
    fn get_epoch_ending_ledger_infos(
        &self, start_epoch: u64, end_epoch: u64,
    ) -> Result<EpochChangeProof>;

    /// See [`DiemDB::get_transactions`].
    ///
    /// [`DiemDB::get_transactions`]:
    /// ../pos-ledger-db/struct.DiemDB.html#method.get_transactions
    fn get_transactions(
        &self, start_version: Version, batch_size: u64,
        ledger_version: Version, fetch_events: bool,
    ) -> Result<TransactionListWithProof>;

    /// See [`DiemDB::get_block_timestamp`].
    ///
    /// [`DiemDB::get_block_timestamp`]:
    /// ../pos-ledger-db/struct.DiemDB.html#method.get_block_timestamp
    fn get_block_timestamp(&self, version: u64) -> Result<u64>;

    /// Gets the version of the last transaction committed before timestamp,
    /// a committed block at or after the required timestamp must exist
    /// (otherwise it's possible the next block committed as a timestamp
    /// smaller than the one in the request).
    fn get_last_version_before_timestamp(
        &self, _timestamp: u64, _ledger_version: Version,
    ) -> Result<Version> {
        unimplemented!()
    }

    /// See [`DiemDB::get_latest_account_state`].
    ///
    /// [`DiemDB::get_latest_account_state`]:
    /// ../pos-ledger-db/struct.DiemDB.html#method.get_latest_account_state
    fn get_latest_account_state(
        &self, address: AccountAddress,
    ) -> Result<Option<AccountStateBlob>>;

    /// Returns the latest ledger info.
    fn get_latest_ledger_info(&self) -> Result<LedgerInfoWithSignatures>;

    /// Returns the latest ledger info.
    fn get_latest_version(&self) -> Result<Version> {
        Ok(self.get_latest_ledger_info()?.ledger_info().version())
    }

    /// Returns the latest version and committed block timestamp
    fn get_latest_commit_metadata(&self) -> Result<(Version, u64)> {
        let ledger_info_with_sig = self.get_latest_ledger_info()?;
        let ledger_info = ledger_info_with_sig.ledger_info();
        Ok((ledger_info.version(), ledger_info.timestamp_usecs()))
    }

    /// Gets information needed from storage during the main node startup.
    /// See [`DiemDB::get_startup_info`].
    ///
    /// [`DiemDB::get_startup_info`]:
    /// ../pos-ledger-db/struct.DiemDB.html#method.get_startup_info
    fn get_startup_info(
        &self, need_pos_state: bool,
    ) -> Result<Option<StartupInfo>>;

    fn get_txn_by_account(
        &self, address: AccountAddress, seq_num: u64, ledger_version: Version,
        fetch_events: bool,
    ) -> Result<Option<TransactionWithProof>>;

    /// Returns proof of new state for a given ledger info with signatures
    /// relative to version known to client
    fn get_state_proof_with_ledger_info(
        &self, known_version: u64, ledger_info: LedgerInfoWithSignatures,
    ) -> Result<(EpochChangeProof, AccumulatorConsistencyProof)>;

    /// Returns proof of new state relative to version known to client
    fn get_state_proof(
        &self, known_version: u64,
    ) -> Result<(
        LedgerInfoWithSignatures,
        EpochChangeProof,
        AccumulatorConsistencyProof,
    )>;

    /// Returns the account state corresponding to the given version and account
    /// address with proof based on `ledger_version`
    fn get_account_state_with_proof(
        &self, address: AccountAddress, version: Version,
        ledger_version: Version,
    ) -> Result<AccountStateWithProof>;

    // Gets an account state by account address, out of the ledger state
    // indicated by the state Merkle tree root with a sparse merkle proof
    // proving state tree root. See [`DiemDB::
    // get_account_state_with_proof_by_version`].
    //
    // [`DiemDB::get_account_state_with_proof_by_version`]:
    // ../pos-ledger-db/struct.DiemDB.html#method.
    // get_account_state_with_proof_by_version
    //
    // This is used by diem core (executor) internally.
    fn get_account_state_with_proof_by_version(
        &self, address: AccountAddress, version: Version,
    ) -> Result<(
        Option<AccountStateBlob>,
        SparseMerkleProof<AccountStateBlob>,
    )>;

    /// See [`DiemDB::get_latest_state_root`].
    ///
    /// [`DiemDB::get_latest_state_root`]:
    /// ../pos-ledger-db/struct.DiemDB.html#method.get_latest_state_root
    fn get_latest_state_root(&self) -> Result<(Version, HashValue)>;

    /// Gets the latest TreeState no matter if db has been bootstrapped.
    /// Used by the Db-bootstrapper.
    fn get_latest_tree_state(&self) -> Result<TreeState>;

    /// Get the ledger info of the epoch that `known_version` belongs to.
    fn get_epoch_ending_ledger_info(
        &self, known_version: u64,
    ) -> Result<LedgerInfoWithSignatures>;

    /// Gets the latest transaction info.
    /// N.B. Unlike get_startup_info(), even if the db is not bootstrapped, this
    /// can return `Some` -- those from a db-restore run.
    fn get_latest_transaction_info_option(
        &self,
    ) -> Result<Option<(Version, TransactionInfo)>> {
        unimplemented!()
    }

    /// Gets the transaction accumulator root hash at specified version.
    /// Caller must guarantee the version is not greater than the latest
    /// version.
    fn get_accumulator_root_hash(
        &self, _version: Version,
    ) -> Result<HashValue> {
        unimplemented!()
    }

    fn get_pos_state(&self, _block_id: &HashValue) -> Result<PosState> {
        unimplemented!()
    }

    fn get_latest_pos_state(&self) -> Arc<PosState> { unimplemented!() }
}

impl MoveStorage for &dyn DbReader {
    fn batch_fetch_resources(
        &self, access_paths: Vec<AccessPath>,
    ) -> Result<Vec<Vec<u8>>> {
        self.batch_fetch_resources_by_version(
            access_paths,
            self.fetch_synced_version()?,
        )
    }

    fn batch_fetch_resources_by_version(
        &self, access_paths: Vec<AccessPath>, version: Version,
    ) -> Result<Vec<Vec<u8>>> {
        let addresses: Vec<AccountAddress> = access_paths
            .iter()
            .collect::<HashSet<_>>()
            .iter()
            .map(|path| path.address)
            .collect();

        let results = addresses
            .iter()
            .map(|addr| {
                self.get_account_state_with_proof_by_version(*addr, version)
            })
            .collect::<Result<Vec<_>>>()?;

        // Account address --> AccountState
        let account_states = addresses
            .iter()
            .zip_eq(results)
            .map(|(addr, (blob, _proof))| {
                let account_state = AccountState::try_from(&blob.ok_or_else(|| {
                    format_err!("missing blob in account state/account does not exist")
                })?)?;
                Ok((addr, account_state))
            })
            .collect::<Result<HashMap<_, AccountState>>>()?;

        access_paths
            .iter()
            .map(|path| {
                Ok(account_states
                    .get(&path.address)
                    .ok_or_else(|| {
                        format_err!(
                            "missing account state for queried access path"
                        )
                    })?
                    .get(&path.path)
                    .ok_or_else(|| {
                        format_err!("no value found in account state")
                    })?
                    .clone())
            })
            .collect()
    }

    fn fetch_synced_version(&self) -> Result<u64> {
        let (synced_version, _) = self
            .get_latest_transaction_info_option()
            .map_err(|e| {
                format_err!(
                    "[MoveStorage] Failed fetching latest transaction info: {}",
                    e
                )
            })?
            .ok_or_else(|| {
                format_err!("[MoveStorage] Latest transaction info not found.")
            })?;
        Ok(synced_version)
    }
}

/// Trait that is implemented by a DB that supports certain public (to client)
/// write APIs expected of a Diem DB. This adds write APIs to DbReader.
pub trait DbWriter: Send + Sync {
    /// Persist transactions. Called by the executor module when either syncing
    /// nodes or committing blocks during normal operation.
    /// See [`DiemDB::save_transactions`].
    ///
    /// [`DiemDB::save_transactions`]:
    /// ../pos-ledger-db/struct.DiemDB.html#method.save_transactions
    fn save_transactions(
        &self, txns_to_commit: &[TransactionToCommit], first_version: Version,
        ledger_info_with_sigs: Option<&LedgerInfoWithSignatures>,
        pos_state: Option<PosState>, committed_blocks: Vec<CommittedBlock>,
        ledger_infos_with_voted_block: Vec<(
            HashValue,
            LedgerInfoWithSignatures,
        )>,
    ) -> Result<()>;

    fn save_reward_event(
        &self, epoch: u64, event: &RewardDistributionEventV2,
    ) -> Result<()>;

    fn delete_pos_state_by_block(&self, block_id: &HashValue) -> Result<()>;
}

#[derive(Clone)]
pub struct DbReaderWriter {
    pub reader: Arc<dyn DbReader>,
    pub writer: Arc<dyn DbWriter>,
}

impl DbReaderWriter {
    pub fn new<D: 'static + DbReader + DbWriter>(db: D) -> Self {
        let reader = Arc::new(db);
        let writer = Arc::clone(&reader);

        Self { reader, writer }
    }

    pub fn from_arc<D: 'static + DbReader + DbWriter>(arc_db: Arc<D>) -> Self {
        let reader = Arc::clone(&arc_db);
        let writer = Arc::clone(&arc_db);

        Self { reader, writer }
    }

    pub fn wrap<D: 'static + DbReader + DbWriter>(db: D) -> (Arc<D>, Self) {
        let arc_db = Arc::new(db);
        (Arc::clone(&arc_db), Self::from_arc(arc_db))
    }
}

impl<D> From<D> for DbReaderWriter
where D: 'static + DbReader + DbWriter
{
    fn from(db: D) -> Self { Self::new(db) }
}

/// Network types for storage service
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum StorageRequest {
    GetAccountStateWithProofByVersionRequest(
        Box<GetAccountStateWithProofByVersionRequest>,
    ),
    GetStartupInfoRequest,
    SaveTransactionsRequest(Box<SaveTransactionsRequest>),
}

#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct GetAccountStateWithProofByVersionRequest {
    /// The access path to query with.
    pub address: AccountAddress,

    /// The version the query is based on.
    pub version: Version,
}

impl GetAccountStateWithProofByVersionRequest {
    /// Constructor.
    pub fn new(address: AccountAddress, version: Version) -> Self {
        Self { address, version }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct SaveTransactionsRequest {
    pub txns_to_commit: Vec<TransactionToCommit>,
    pub first_version: Version,
    pub ledger_info_with_signatures: Option<LedgerInfoWithSignatures>,
}

impl SaveTransactionsRequest {
    /// Constructor.
    pub fn new(
        txns_to_commit: Vec<TransactionToCommit>, first_version: Version,
        ledger_info_with_signatures: Option<LedgerInfoWithSignatures>,
    ) -> Self {
        SaveTransactionsRequest {
            txns_to_commit,
            first_version,
            ledger_info_with_signatures,
        }
    }
}

pub trait DBReaderForPoW: Send + Sync + DbReader {
    fn get_latest_ledger_info_option(&self)
        -> Option<LedgerInfoWithSignatures>;

    /// TODO(lpl): It's possible to use round number?
    fn get_block_ledger_info(
        &self, consensus_block_id: &HashValue,
    ) -> Result<LedgerInfoWithSignatures>;

    fn get_events_by_version(
        &self, start_version: u64, end_version: u64,
    ) -> Result<Vec<ContractEvent>>;

    fn get_epoch_ending_blocks(
        &self, start_epoch: u64, end_epoch: u64,
    ) -> Result<Vec<HashValue>>;

    fn get_reward_event(&self, epoch: u64)
        -> Result<RewardDistributionEventV2>;

    fn get_committed_block_by_hash(
        &self, block_hash: &HashValue,
    ) -> Result<CommittedBlock>;

    fn get_committed_block_hash_by_view(&self, view: u64) -> Result<HashValue>;

    fn get_ledger_info_by_voted_block(
        &self, block_id: &HashValue,
    ) -> Result<LedgerInfoWithSignatures>;

    fn get_block_hash_by_epoch_and_round(
        &self, epoch: u64, round: u64,
    ) -> Result<HashValue>;
}
