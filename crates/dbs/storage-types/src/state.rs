// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

use cfx_internal_common::StateRootWithAuxInfo;
use primitives::{EpochId, StorageKeyWithSpace};

use crate::{Error, Result};

pub type MptKeyValue = (Vec<u8>, Box<[u8]>);

pub trait StateTrait: Sync + Send {
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
