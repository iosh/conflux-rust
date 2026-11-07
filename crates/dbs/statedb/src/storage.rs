// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

use cfx_storage_types::{MptKeyValue, Result, StateTrait};
use primitives::StorageKeyWithSpace;

/// Read/write storage used by the StateDb buffer.
///
/// Existing node backends keep implementing `StateTrait`; their owning box is
/// adapted here. Private candidates can implement this contract without
/// computing roots or committing epochs. A failed writeback may leave partial
/// backend changes, so the caller must discard a failed private candidate.
pub trait StateStorage: Sync + Send {
    fn get(&self, key: StorageKeyWithSpace<'_>) -> Result<Option<Box<[u8]>>>;

    fn set(
        &mut self, key: StorageKeyWithSpace<'_>, value: Box<[u8]>,
    ) -> Result<()>;

    fn delete(&mut self, key: StorageKeyWithSpace<'_>) -> Result<()>;

    /// Exposes the backend's test-only removal operation.
    fn delete_test_only(
        &mut self, key: StorageKeyWithSpace<'_>,
    ) -> Result<Option<Box<[u8]>>>;

    fn delete_all(
        &mut self, prefix: StorageKeyWithSpace<'_>,
    ) -> Result<Option<Vec<MptKeyValue>>>;

    fn read_all(
        &mut self, prefix: StorageKeyWithSpace<'_>,
    ) -> Result<Option<Vec<MptKeyValue>>>;

    fn read_all_with_callback(
        &mut self, prefix: StorageKeyWithSpace<'_>,
        callback: &mut dyn FnMut(MptKeyValue), only_account_key: bool,
    ) -> Result<()>;
}

impl StateStorage for Box<dyn StateTrait> {
    fn get(&self, key: StorageKeyWithSpace<'_>) -> Result<Option<Box<[u8]>>> {
        StateTrait::get(self.as_ref(), key)
    }

    fn set(
        &mut self, key: StorageKeyWithSpace<'_>, value: Box<[u8]>,
    ) -> Result<()> {
        StateTrait::set(self.as_mut(), key, value)
    }

    fn delete(&mut self, key: StorageKeyWithSpace<'_>) -> Result<()> {
        StateTrait::delete(self.as_mut(), key)
    }

    fn delete_test_only(
        &mut self, key: StorageKeyWithSpace<'_>,
    ) -> Result<Option<Box<[u8]>>> {
        StateTrait::delete_test_only(self.as_mut(), key)
    }

    fn delete_all(
        &mut self, prefix: StorageKeyWithSpace<'_>,
    ) -> Result<Option<Vec<MptKeyValue>>> {
        StateTrait::delete_all(self.as_mut(), prefix)
    }

    fn read_all(
        &mut self, prefix: StorageKeyWithSpace<'_>,
    ) -> Result<Option<Vec<MptKeyValue>>> {
        StateTrait::read_all(self.as_mut(), prefix)
    }

    fn read_all_with_callback(
        &mut self, prefix: StorageKeyWithSpace<'_>,
        callback: &mut dyn FnMut(MptKeyValue), only_account_key: bool,
    ) -> Result<()> {
        StateTrait::read_all_with_callback(
            self.as_mut(),
            prefix,
            callback,
            only_account_key,
        )
    }
}

impl<T: StateStorage + ?Sized> StateStorage for &mut T {
    fn get(&self, key: StorageKeyWithSpace<'_>) -> Result<Option<Box<[u8]>>> {
        T::get(&**self, key)
    }

    fn set(
        &mut self, key: StorageKeyWithSpace<'_>, value: Box<[u8]>,
    ) -> Result<()> {
        T::set(&mut **self, key, value)
    }

    fn delete(&mut self, key: StorageKeyWithSpace<'_>) -> Result<()> {
        T::delete(&mut **self, key)
    }

    fn delete_test_only(
        &mut self, key: StorageKeyWithSpace<'_>,
    ) -> Result<Option<Box<[u8]>>> {
        T::delete_test_only(&mut **self, key)
    }

    fn delete_all(
        &mut self, prefix: StorageKeyWithSpace<'_>,
    ) -> Result<Option<Vec<MptKeyValue>>> {
        T::delete_all(&mut **self, prefix)
    }

    fn read_all(
        &mut self, prefix: StorageKeyWithSpace<'_>,
    ) -> Result<Option<Vec<MptKeyValue>>> {
        T::read_all(&mut **self, prefix)
    }

    fn read_all_with_callback(
        &mut self, prefix: StorageKeyWithSpace<'_>,
        callback: &mut dyn FnMut(MptKeyValue), only_account_key: bool,
    ) -> Result<()> {
        T::read_all_with_callback(
            &mut **self,
            prefix,
            callback,
            only_account_key,
        )
    }
}
