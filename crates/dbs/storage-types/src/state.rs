// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

use cfx_internal_common::StateRootWithAuxInfo;
use cfx_types::{AddressWithSpace, U256};
use primitives::{Account, EpochId, StorageKey, StorageKeyWithSpace};
use rlp::Rlp;

use crate::{Error, Result};

pub type MptKeyValue = (Vec<u8>, Box<[u8]>);

pub trait StateTrait: Sync + Send {
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

    // Finalize
    /// It's costly to compute state root however it's only necessary to compute
    /// state root once before committing.
    fn compute_state_root(&mut self) -> Result<StateRootWithAuxInfo>;
    fn get_state_root(&self) -> Result<StateRootWithAuxInfo>;
    fn commit(&mut self, epoch: EpochId) -> Result<StateRootWithAuxInfo>;
}
