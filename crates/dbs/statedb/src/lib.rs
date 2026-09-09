// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

#[macro_use]
extern crate cfx_util_macros;
#[macro_use]
extern crate log;

pub mod global_params;
#[cfg(feature = "testonly_code")]
mod in_memory_storage;
mod statedb_ext;
use cfx_types::H256;
use primitives::StorageValue;

use cfx_db_errors::statedb as error;

#[cfg(test)]
mod tests;

pub use self::{
    error::{Error, Result},
    impls::StateDb as StateDbGeneric,
    statedb_ext::StateDbExt,
};
pub use cfx_storage_types::access_mode;
pub type StateDb = StateDbGeneric;

// Put StateDb in mod to make sure that methods from statedb_ext don't access
// its fields directly.
mod impls {
    type Key = Vec<u8>;
    type Value = Option<Arc<[u8]>>;

    // Use BTreeMap so that we can delete ranges efficiently
    // see `delete_all`
    type AccessedEntries = BTreeMap<Key, EntryValue>;

    // Use generic type for better test-ability.
    pub struct StateDb {
        /// Contains the original storage key values for all loaded and
        /// modified key values.
        accessed_entries: RwLock<AccessedEntries>,

        /// Account storage/code clears applied before later point writes.
        account_clears: BTreeSet<AccountClear>,

        /// The underlying storage, The storage is updated only upon fn
        /// commit().
        storage: Box<dyn StorageStateTrait>,
    }

    impl StateDb {
        pub fn new(storage: Box<dyn StorageStateTrait>) -> Self {
            StateDb {
                accessed_entries: Default::default(),
                account_clears: BTreeSet::new(),
                storage,
            }
        }

        #[cfg(feature = "testonly_code")]
        pub fn new_for_unit_test() -> Self {
            use self::in_memory_storage::InmemoryStorage;

            Self::new(Box::new(InmemoryStorage::default()))
        }

        #[cfg(feature = "testonly_code")]
        pub fn new_for_unit_test_with_epoch(epoch_id: &EpochId) -> Self {
            use self::in_memory_storage::InmemoryStorage;

            Self::new(Box::new(
                InmemoryStorage::from_epoch_id(epoch_id).unwrap(),
            ))
        }

        #[cfg(test)]
        pub fn get_from_cache(&self, key: &Vec<u8>) -> Value {
            self.accessed_entries
                .read()
                .get(key)
                .and_then(|v| v.current_value.clone())
        }

        fn is_key_cleared(&self, key: StorageKeyWithSpace) -> bool {
            AccountClear::contains_key(&self.account_clears, key)
        }

        /// Reads a balance, giving pending account values and deletions
        /// precedence over the underlying storage.
        ///
        /// This read does not cache a partial account or infer account absence
        /// from a zero balance.
        ///
        /// # Errors
        ///
        /// Returns an account decoding or storage read error.
        pub fn get_account_balance(
            &self, address: &AddressWithSpace,
        ) -> Result<U256> {
            let key = StorageKey::new_account_key(&address.address)
                .with_space(address.space)
                .to_key_bytes();
            if let Some(entry) = self.accessed_entries.read().get(&key) {
                return match &entry.current_value {
                    None => Ok(U256::zero()),
                    Some(raw) => Ok(Account::new_from_rlp(
                        address.address,
                        &rlp::Rlp::new(raw),
                    )?
                    .balance),
                };
            }
            Ok(self.storage.get_account_balance(address)?)
        }

        /// Update the accessed_entries while getting the value.
        pub(crate) fn get_raw(
            &self, key: StorageKeyWithSpace,
        ) -> Result<Option<Arc<[u8]>>> {
            let key_bytes = key.to_key_bytes();
            let r;
            let accessed_entries_read_guard = self.accessed_entries.read();
            if let Some(v) = accessed_entries_read_guard.get(&key_bytes) {
                r = v.current_value.clone();
            } else {
                drop(accessed_entries_read_guard);
                let mut accessed_entries = self.accessed_entries.write();
                let entry = accessed_entries.entry(key_bytes);
                if let Occupied(o) = &entry {
                    r = o.get().current_value.clone();
                } else {
                    r = if self.is_key_cleared(key) {
                        None
                    } else {
                        self.storage.get(key)?.map(Into::into)
                    };
                    entry.or_insert(EntryValue::new(r.clone()));
                };
            };
            trace!("get_raw key={:?}, value={:?}", key, r);
            Ok(r)
        }

        #[cfg(feature = "testonly_code")]
        pub fn get_raw_test(
            &self, key: StorageKeyWithSpace,
        ) -> Result<Option<Arc<[u8]>>> {
            self.get_raw(key)
        }

        /// Set the value under `key` to `value` in `accessed_entries`.
        /// This method will read from db if `key` is not present.
        /// This method will also update the latest checkpoint if necessary.
        fn modify_single_value(
            &mut self, key: StorageKeyWithSpace, value: Option<Box<[u8]>>,
        ) -> Result<()> {
            let key_bytes = key.to_key_bytes();
            let key_was_cleared = self.is_key_cleared(key);
            let mut entry =
                self.accessed_entries.get_mut().entry(key_bytes.clone());
            let value = value.map(Into::into);

            match &mut entry {
                Occupied(o) => {
                    // set `current_value` to `value` and keep the old value
                    Some(std::mem::replace(
                        &mut o.get_mut().current_value,
                        value,
                    ))
                }

                // Vacant
                &mut Vacant(_) => {
                    let original_value = if key_was_cleared {
                        None
                    } else {
                        self.storage.get(key)?.map(Into::into)
                    };

                    entry.or_insert(EntryValue::new_modified(
                        original_value,
                        value,
                    ));

                    None
                }
            };

            Ok(())
        }

        pub(crate) fn set_raw(
            &mut self, key: StorageKeyWithSpace, value: Box<[u8]>,
            debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<()> {
            if let Some(record) = debug_record {
                record.state_ops.push(StateOp::StorageLevelOp {
                    op_name: "set".into(),
                    key: key.to_key_bytes(),
                    maybe_value: Some(value.clone().into()),
                })
            }

            self.modify_single_value(key, Some(value))
        }

        pub fn delete(
            &mut self, key: StorageKeyWithSpace,
            debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<()> {
            if let Some(record) = debug_record {
                record.state_ops.push(StateOp::StorageLevelOp {
                    op_name: "delete".into(),
                    key: key.to_key_bytes(),
                    maybe_value: None,
                })
            }

            self.modify_single_value(key, None)
        }

        /// Clears an account's storage entries and layout in its space.
        ///
        /// Later writes take precedence over the clear. Account metadata and
        /// collateral accounting remain the caller's responsibility.
        ///
        /// # Errors
        ///
        /// Enumerating backends can fail while reading old entries. Deferred
        /// backend errors surface when changes are applied; a failed candidate
        /// must be discarded.
        pub fn clear_account_storage(
            &mut self, address: &AddressWithSpace,
            debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<()> {
            self.clear_account_data(
                AccountClear::Storage(*address),
                debug_record,
            )
        }

        /// Clears all code records belonging to an account in its space.
        ///
        /// Later code writes take precedence over the clear. Updating the
        /// account's code hash and settling collateral remain the caller's
        /// responsibility.
        ///
        /// # Errors
        ///
        /// Enumerating backends can fail while reading old entries. Deferred
        /// backend errors surface when changes are applied; a failed candidate
        /// must be discarded.
        pub fn clear_account_code(
            &mut self, address: &AddressWithSpace,
            debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<()> {
            self.clear_account_data(AccountClear::Code(*address), debug_record)
        }

        fn clear_account_data(
            &mut self, clear: AccountClear,
            debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<()> {
            let key_prefix = clear.key_prefix();
            if self.storage.account_clear_mode() == AccountClearMode::Enumerate
            {
                return self
                    .delete_all::<access_mode::Write>(key_prefix, debug_record)
                    .map(|_| ());
            }

            let key_bytes = key_prefix.to_key_bytes();
            if let Some(record) = debug_record {
                record.state_ops.push(StateOp::StorageLevelOp {
                    op_name: match clear {
                        AccountClear::Storage(_) => "clear_account_storage",
                        AccountClear::Code(_) => "clear_account_code",
                    }
                    .into(),
                    key: key_bytes.clone(),
                    maybe_value: None,
                });
            }

            let upper_bound = to_key_prefix_iter_upper_bound(&key_bytes);
            let accessed_entries = self.accessed_entries.get_mut();
            let keys_to_remove: Vec<_> = accessed_entries
                .range::<[u8], _>((
                    Included(key_bytes.as_slice()),
                    upper_bound.as_deref().map_or(Unbounded, Excluded),
                ))
                .map(|(key, _)| key.clone())
                .collect();
            for key in keys_to_remove {
                accessed_entries.remove(&key);
            }

            self.account_clears.insert(clear);
            Ok(())
        }

        pub fn read_all(
            &mut self, key_prefix: StorageKeyWithSpace,
            debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<Vec<MptKeyValue>> {
            self.delete_all::<access_mode::Read>(key_prefix, debug_record)
        }

        /// Visits the effective view, including pending writes and deletions.
        ///
        /// Backend entries shadowed by local entries or account clears are
        /// omitted. Values are passed to the callback without collecting the
        /// complete range first.
        ///
        /// # Errors
        ///
        /// Returns a storage error if the remaining backend range cannot be
        /// read. The callback may already have received entries on failure.
        pub fn read_all_with_callback(
            &mut self, access_key_prefix: StorageKeyWithSpace,
            callback: &mut dyn FnMut(MptKeyValue), only_account_key: bool,
        ) -> Result<()> {
            let range_was_cleared = self.is_key_cleared(access_key_prefix);
            let key_bytes = access_key_prefix.to_key_bytes();
            let account_clears = &self.account_clears;
            let accessed_entries = self.accessed_entries.get_mut();

            if !range_was_cleared {
                self.storage.read_all_with_callback(
                    access_key_prefix,
                    &mut |(key, value)| {
                        if !key.starts_with(&key_bytes)
                            || accessed_entries.contains_key(&key)
                        {
                            return;
                        }
                        if only_account_key || !account_clears.is_empty() {
                            let storage_key =
                                StorageKeyWithSpace::from_key_bytes::<
                                    SkipInputCheck,
                                >(&key);
                            if (only_account_key
                                && !matches!(
                                    storage_key.key,
                                    StorageKey::AccountKey(_)
                                ))
                                || AccountClear::contains_key(
                                    account_clears,
                                    storage_key,
                                )
                            {
                                return;
                            }
                        }
                        callback((key, value));
                    },
                    only_account_key,
                )?;
            }

            let upper_bound = to_key_prefix_iter_upper_bound(&key_bytes);
            for (key, entry) in accessed_entries.range::<[u8], _>((
                Included(key_bytes.as_slice()),
                upper_bound.as_deref().map_or(Unbounded, Excluded),
            )) {
                if let Some(value) = &entry.current_value {
                    if only_account_key
                        && !matches!(
                            StorageKeyWithSpace::from_key_bytes::<SkipInputCheck>(
                                key,
                            )
                            .key,
                            StorageKey::AccountKey(_)
                        )
                    {
                        continue;
                    }
                    callback((key.clone(), (&**value).into()));
                }
            }
            Ok(())
        }

        pub fn delete_all<AM: access_mode::AccessMode>(
            &mut self, key_prefix: StorageKeyWithSpace,
            debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<Vec<MptKeyValue>> {
            let key_bytes = key_prefix.to_key_bytes();
            let range_was_cleared = self.is_key_cleared(key_prefix);
            if let Some(record) = debug_record {
                record.state_ops.push(StateOp::StorageLevelOp {
                    op_name: if AM::READ_ONLY {
                        "iterate"
                    } else {
                        "delete_all"
                    }
                    .into(),
                    key: key_bytes.clone(),
                    maybe_value: None,
                })
            }
            let account_clears = &self.account_clears;
            let accessed_entries = self.accessed_entries.get_mut();
            // First, all new keys in the subtree shall be deleted.
            let iter_range_upper_bound =
                to_key_prefix_iter_upper_bound(&key_bytes);
            let iter_range = match &iter_range_upper_bound {
                None => accessed_entries
                    .range_mut::<[u8], _>((Included(&*key_bytes), Unbounded)),

                // delete_all will not delete any key which doesn't exist before
                // the operation. Therefore we don't need to
                // check the accessed_entries prior to the
                // operation.
                Some(upper_bound) => accessed_entries.range_mut::<[u8], _>((
                    Included(&*key_bytes),
                    Excluded(&**upper_bound),
                )),
            };
            let mut deleted_kvs = vec![];
            for (k, v) in iter_range {
                if v.current_value != None {
                    deleted_kvs.push((
                        k.clone(),
                        (&**v.current_value.as_ref().unwrap()).into(),
                    ));
                    if !AM::READ_ONLY {
                        v.current_value = None;
                    }
                }
            }
            // Then, remove all un-modified existing keys.
            let deleted = if range_was_cleared {
                None
            } else {
                self.storage.read_all(key_prefix)?
            };
            // We must update the accessed_entries.
            if let Some(storage_deleted) = &deleted {
                for (k, v) in storage_deleted {
                    if !account_clears.is_empty()
                        && AccountClear::contains_key(
                            account_clears,
                            StorageKeyWithSpace::from_key_bytes::<SkipInputCheck>(
                                k,
                            ),
                        )
                    {
                        continue;
                    }
                    let entry = accessed_entries.entry(k.clone());
                    let was_vacant = if let Occupied(_) = &entry {
                        // Nothing to do for existing entry, because we have
                        // already scanned through accessed_entries.
                        false
                    } else {
                        true
                    };
                    if was_vacant {
                        deleted_kvs.push((k.clone(), v.clone()));
                        if !AM::READ_ONLY {
                            entry.or_insert(EntryValue::new_modified(
                                Some((&**v).into()),
                                None,
                            ));
                        }
                    }
                }
            }

            Ok(deleted_kvs)
        }

        pub fn get_account_storage_entries(
            &mut self, address: &AddressWithSpace,
            debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<BTreeMap<cfx_types::StorageKey, cfx_types::StorageValue>>
        {
            let mut storage = BTreeMap::new();

            let storage_prefix =
                StorageKey::new_storage_root_key(&address.address)
                    .with_space(address.space);

            let kv_pairs = self.read_all(storage_prefix, debug_record)?;
            for (key, value) in kv_pairs {
                let storage_key_with_space =
                    StorageKeyWithSpace::from_key_bytes::<SkipInputCheck>(&key);
                if let StorageKey::StorageKey {
                    address_bytes: _,
                    storage_key,
                } = storage_key_with_space.key
                {
                    let h256_storage_key = H256::from_slice(storage_key);
                    let storage_value_with_owner: StorageValue =
                        rlp::decode(&value)?;
                    storage.insert(
                        h256_storage_key,
                        storage_value_with_owner.value,
                    );
                } else {
                    trace!("Not an storage key: {:?}", storage_key_with_space);
                    continue;
                }
            }

            Ok(storage)
        }

        /// Load the storage layout for state commits.
        /// Modification to storage layout is the same as modification of
        /// any other key-values. But as required by MPT structure we
        /// must commit storage layout for any storage changes under the
        /// same account. To load the storage layout, we first load from
        /// the local changes (i.e. accessed_entries), then from the
        /// storage if it's untouched.
        fn load_storage_layout(
            storage_layouts_to_rewrite: &mut HashMap<
                (Vec<u8>, Space),
                StorageLayout,
            >,
            accept_account_deletion: bool, address: &[u8], space: Space,
            storage: &dyn StorageStateTrait,
            accessed_entries: &AccessedEntries,
        ) -> Result<()> {
            if storage_layouts_to_rewrite
                .contains_key(&(address.to_vec(), space))
            {
                return Ok(());
            }
            let storage_layout_key =
                StorageKey::StorageRootKey(address).with_space(space);
            let current_storage_layout = match accessed_entries
                .get(&storage_layout_key.to_key_bytes())
            {
                Some(entry) => match &entry.current_value {
                    // We don't rewrite storage layout for account to
                    // delete.
                    None => {
                        if accept_account_deletion {
                            return Ok(());
                        } else {
                            // This is defensive checking, against certain
                            // cases when we are not deleting the account
                            // for sure.
                            bail!(Error::IncompleteDatabase(
                                Address::from_slice(address)
                            ));
                        }
                    }

                    Some(value_ref) => StorageLayout::from_bytes(&*value_ref)?,
                },
                None => match storage.get(storage_layout_key)? {
                    // A new account must set StorageLayout before accessing
                    // the storage.
                    None => bail!(Error::IncompleteDatabase(
                        Address::from_slice(address)
                    )),
                    Some(raw) => StorageLayout::from_bytes(raw.as_ref())?,
                },
            };
            storage_layouts_to_rewrite
                .insert((address.into(), space), current_storage_layout);

            Ok(())
        }

        pub fn set_storage_layout(
            &mut self, address: &AddressWithSpace,
            storage_layout: StorageLayout,
            debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<()> {
            self.set_raw(
                StorageKey::new_storage_root_key(&address.address)
                    .with_space(address.space),
                storage_layout.to_bytes().into_boxed_slice(),
                debug_record,
            )
        }

        /// storage_layout is special, because it must always present if there
        /// is any storage value changed.
        fn commit_storage_layout(
            &mut self, address: &[u8], space: Space, layout: &StorageLayout,
            debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<()> {
            let key = StorageKey::StorageRootKey(address).with_space(space);
            let value = layout.to_bytes().into_boxed_slice();
            if let Some(record) = debug_record {
                record.state_ops.push(StateOp::StorageLevelOp {
                    op_name: "commit_storage_layout".into(),
                    key: key.to_key_bytes(),
                    maybe_value: Some(value.clone().into()),
                })
            };
            Ok(self.storage.set(key, value)?)
        }

        fn apply_changes_to_storage(
            &mut self, mut debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<()> {
            for clear in &self.account_clears {
                match clear {
                    AccountClear::Storage(address) => {
                        self.storage.clear_account_storage(address)?
                    }
                    AccountClear::Code(address) => {
                        self.storage.clear_account_code(address)?
                    }
                }
            }

            let mut storage_layouts_to_rewrite = Default::default();
            let accessed_entries = &*self.accessed_entries.get_mut();
            // First of all, apply all changes to the underlying storage.
            for (k, v) in accessed_entries {
                if !v.is_modified() {
                    continue;
                }

                let storage_key =
                    StorageKeyWithSpace::from_key_bytes::<SkipInputCheck>(k);
                match &v.current_value {
                    Some(v) => {
                        self.storage.set(storage_key, (&**v).into())?;
                    }
                    None => {
                        self.storage.delete(storage_key)?;
                    }
                }

                match &storage_key.key {
                    StorageKey::StorageKey { address_bytes, .. } => {
                        Self::load_storage_layout(
                            &mut storage_layouts_to_rewrite,
                            /* accept_account_deletion = */
                            v.current_value.is_none(),
                            address_bytes,
                            storage_key.space,
                            self.storage.as_ref(),
                            &accessed_entries,
                        )?;
                    }
                    StorageKey::AccountKey(address_bytes)
                        if (address_bytes.is_builtin_address()
                            || address_bytes.is_contract_address()
                            || storage_key.space == Space::Ethereum)
                            && v.original_value.is_none() =>
                    {
                        let result = Self::load_storage_layout(
                            &mut storage_layouts_to_rewrite,
                            /* accept_account_deletion = */ false,
                            address_bytes,
                            storage_key.space,
                            self.storage.as_ref(),
                            &accessed_entries,
                        );
                        if result.is_err() {
                            debug!(
                                "Contract address {:?} (space {:?}) is created without storage_layout. \
                                It's probably created by a balance transfer.",
                                Address::from_slice(address_bytes), storage_key.space
                            );
                        }
                    }
                    StorageKey::CodeKey { address_bytes, .. }
                        if v.original_value.is_none() =>
                    {
                        Self::load_storage_layout(
                            &mut storage_layouts_to_rewrite,
                            /* accept_account_deletion = */ false,
                            address_bytes,
                            storage_key.space,
                            self.storage.as_ref(),
                            &accessed_entries,
                        )?;
                    }
                    _ => {}
                }
            }
            // Set storage layout for contracts with storage modification or
            // contracts with storage_layout initialization or modification.
            for ((k, space), v) in &storage_layouts_to_rewrite {
                self.commit_storage_layout(
                    k,
                    *space,
                    v,
                    debug_record.as_deref_mut(),
                )?;
            }
            // Mark all modification applied.
            self.accessed_entries = Default::default();
            self.account_clears.clear();
            Ok(())
        }

        /// This method is only used for genesis block because state root is
        /// required to compute genesis epoch_id. For other blocks there are
        /// deferred execution so the state root computation is merged inside
        /// commit method.
        pub fn compute_state_root(
            &mut self, debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<StateRootWithAuxInfo> {
            self.apply_changes_to_storage(debug_record)?;
            Ok(self.storage.compute_state_root()?)
        }

        pub fn commit(
            &mut self, epoch_id: EpochId,
            mut debug_record: Option<&mut ComputeEpochDebugRecord>,
        ) -> Result<StateRootWithAuxInfo> {
            self.apply_changes_to_storage(debug_record.as_deref_mut())?;

            let result = match self.storage.get_state_root() {
                Ok(r) => r,
                Err(_) => self.compute_state_root(debug_record)?,
            };

            self.storage.commit(epoch_id)?;

            Ok(result)
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum AccountClear {
        Storage(AddressWithSpace),
        Code(AddressWithSpace),
    }

    impl AccountClear {
        fn contains_key(
            clears: &BTreeSet<Self>, key: StorageKeyWithSpace,
        ) -> bool {
            if clears.is_empty() {
                return false;
            }
            let clear = match key.key {
                StorageKey::StorageRootKey(address_bytes)
                | StorageKey::StorageKey { address_bytes, .. } => {
                    Self::Storage(AddressWithSpace {
                        address: Address::from_slice(address_bytes),
                        space: key.space,
                    })
                }
                StorageKey::CodeRootKey(address_bytes)
                | StorageKey::CodeKey { address_bytes, .. } => {
                    Self::Code(AddressWithSpace {
                        address: Address::from_slice(address_bytes),
                        space: key.space,
                    })
                }
                _ => return false,
            };
            clears.contains(&clear)
        }

        fn key_prefix(&self) -> StorageKeyWithSpace<'_> {
            match self {
                Self::Storage(address) => {
                    StorageKey::new_storage_root_key(&address.address)
                        .with_space(address.space)
                }
                Self::Code(address) => {
                    StorageKey::new_code_root_key(&address.address)
                        .with_space(address.space)
                }
            }
        }
    }

    struct EntryValue {
        original_value: Value,
        current_value: Value,
    }

    impl EntryValue {
        fn new(value: Value) -> Self {
            let value_clone = value.clone();
            Self {
                original_value: value,
                current_value: value_clone,
            }
        }

        fn new_modified(original_value: Value, current_value: Value) -> Self {
            Self {
                original_value,
                current_value,
            }
        }

        fn is_modified(&self) -> bool {
            self.original_value.ne(&self.current_value)
        }
    }

    use super::*;
    use cfx_internal_common::{
        debug::{ComputeEpochDebugRecord, StateOp},
        StateRootWithAuxInfo,
    };
    use cfx_storage_types::{
        access_mode, to_key_prefix_iter_upper_bound, AccountClearMode,
        MptKeyValue, StateTrait as StorageStateTrait,
    };
    use cfx_types::{
        address_util::AddressUtil, Address, AddressWithSpace, Space, U256,
    };
    use hashbrown::HashMap;
    use parking_lot::RwLock;
    use primitives::{
        Account, EpochId, SkipInputCheck, StorageKey, StorageKeyWithSpace,
        StorageLayout,
    };
    use std::{
        collections::{
            btree_map::Entry::{Occupied, Vacant},
            BTreeMap, BTreeSet,
        },
        ops::Bound::{Excluded, Included, Unbounded},
        sync::Arc,
    };
}
