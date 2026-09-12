// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

use cfx_internal_common::StateRootWithAuxInfo;
use cfx_types::{AddressWithSpace, U256};
use primitives::{Account, EpochId, StorageKey, StorageKeyWithSpace};
use rlp::Rlp;

use crate::{Error, Result};

pub type MptKeyValue = (Vec<u8>, Box<[u8]>);

/// How StateDb buffers account storage and code clears for a backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountClearMode {
    /// Enumerate existing entries and buffer individual deletions.
    Enumerate,
    /// Buffer account clears and apply them before subsequent point writes.
    Deferred,
}

/// State reads and writes used during execution.
///
/// Implementations need not compute a state root or commit an epoch.
pub trait StateStorage: Sync + Send {
    /// Reads a balance without requiring the caller to load a complete account.
    ///
    /// The default implementation decodes the stored account. Backends with
    /// field-level data sources may override it to read only the balance from
    /// the same state view. A zero balance does not establish account absence.
    ///
    /// # Errors
    ///
    /// Returns a storage or account decoding error if the balance cannot be
    /// read.
    fn get_account_balance(&self, address: &AddressWithSpace) -> Result<U256> {
        match self.get(
            StorageKey::new_account_key(&address.address)
                .with_space(address.space),
        )? {
            None => Ok(U256::zero()),
            Some(raw) => {
                Ok(Account::new_from_rlp(address.address, &Rlp::new(&raw))?
                    .balance)
            }
        }
    }

    /// Selects how StateDb buffers account clears. The mode must remain
    /// unchanged for the lifetime of this backend.
    ///
    /// Deferred backends must override both account clear methods without
    /// enumeration. Clears must hide old entries from subsequent reads while
    /// allowing later writes to remain visible.
    fn account_clear_mode(&self) -> AccountClearMode {
        AccountClearMode::Enumerate
    }

    // Actions.
    fn get(&self, access_key: StorageKeyWithSpace)
        -> Result<Option<Box<[u8]>>>;
    fn set(
        &mut self, access_key: StorageKeyWithSpace, value: Box<[u8]>,
    ) -> Result<()>;
    fn delete(&mut self, access_key: StorageKeyWithSpace) -> Result<()>;
    fn delete_test_only(
        &mut self, access_key: StorageKeyWithSpace,
    ) -> Result<Option<Box<[u8]>>>;
    // Delete everything prefixed by access_key and return deleted key value
    // pairs.
    fn delete_all(
        &mut self, access_key_prefix: StorageKeyWithSpace,
    ) -> Result<Option<Vec<MptKeyValue>>>;
    /// Clears an account's storage entries and storage layout in its space.
    ///
    /// The default implementation enumerates through `delete_all`. Deferred
    /// backends can mask the old storage locally. Later writes take precedence
    /// over the clear; callers must restore the layout before writing storage.
    /// Account metadata and collateral accounting remain the caller's
    /// responsibility.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the clear cannot be applied.
    fn clear_account_storage(
        &mut self, address: &AddressWithSpace,
    ) -> Result<()> {
        self.delete_all(
            StorageKey::new_storage_root_key(&address.address)
                .with_space(address.space),
        )
        .map(|_| ())
    }

    /// Clears all code records belonging to an account in its space.
    ///
    /// The default implementation enumerates through `delete_all`. Deferred
    /// backends can mask the old code locally. Later code writes take
    /// precedence over the clear. Updating the account's code hash and
    /// settling collateral remain the caller's responsibility.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the clear cannot be applied.
    fn clear_account_code(&mut self, address: &AddressWithSpace) -> Result<()> {
        self.delete_all(
            StorageKey::new_code_root_key(&address.address)
                .with_space(address.space),
        )
        .map(|_| ())
    }

    // TODO: Remove this mut.
    fn read_all(
        &mut self, access_key_prefix: StorageKeyWithSpace,
    ) -> Result<Option<Vec<MptKeyValue>>>;

    fn read_all_with_callback(
        &mut self, _access_key_prefix: StorageKeyWithSpace,
        _callback: &mut dyn FnMut(MptKeyValue), _only_account_key: bool,
    ) -> Result<()> {
        Err(Error::Msg("Not implemented".into()))
    }
}

/// State storage that can compute roots and commit epochs.
pub trait StateTrait: StateStorage {
    // Finalize
    /// It's costly to compute state root however it's only necessary to compute
    /// state root once before committing.
    fn compute_state_root(&mut self) -> Result<StateRootWithAuxInfo>;
    fn get_state_root(&self) -> Result<StateRootWithAuxInfo>;
    fn commit(&mut self, epoch: EpochId) -> Result<StateRootWithAuxInfo>;
}
