use crate::state::overlay_account::AccountEntry;

use super::State;
use cfx_internal_common::debug::ComputeEpochDebugRecord;
use cfx_statedb::Result as DbResult;
use cfx_types::AddressWithSpace;
use primitives::{Account, StorageKey};

impl State<'_> {
    /// Writes account and global changes through StateDb to its backend,
    /// returning the account updates used for transaction-pool notification.
    ///
    /// This does not compute a state root or commit an epoch. Release the
    /// State before finalizing storage borrowed from the caller.
    ///
    /// # Errors
    ///
    /// Returns a database error if a change cannot be applied. The backend may
    /// contain partial writes, so the caller must abandon the working state
    /// rather than publish it.
    ///
    /// # Panics
    ///
    /// Panics if a checkpoint is still active.
    pub fn apply_changes_to_storage(
        &mut self, mut debug_record: Option<&mut ComputeEpochDebugRecord>,
    ) -> DbResult<Vec<Account>> {
        let accounts_for_txpool =
            self.apply_changes_to_statedb(debug_record.as_deref_mut())?;
        self.db.apply_changes_to_storage(debug_record)?;
        Ok(accounts_for_txpool)
    }

    /// Apply changes for the accounts and global variables to the statedb.
    pub fn apply_changes_to_statedb(
        &mut self, mut debug_record: Option<&mut ComputeEpochDebugRecord>,
    ) -> DbResult<Vec<Account>> {
        debug!("state.commit_changes");

        let accounts_for_txpool =
            self.commit_dirty_accounts(debug_record.as_deref_mut())?;
        self.global_stat.commit(&mut self.db, debug_record)?;
        Ok(accounts_for_txpool)
    }

    fn commit_dirty_accounts(
        &mut self, mut debug_record: Option<&mut ComputeEpochDebugRecord>,
    ) -> DbResult<Vec<Account>> {
        assert!(self.no_checkpoint());

        self.commit_cache(false);
        let cache_items = self.committed_cache.drain();
        let mut to_commit_accounts = cache_items
            .filter_map(|(_, acc)| acc.into_to_commit_account())
            .collect::<Vec<_>>();
        to_commit_accounts.sort_by(|a, b| a.address().cmp(b.address()));

        let mut accounts_to_notify = vec![];

        for account in to_commit_accounts.into_iter() {
            let address = *account.address();

            if account.pending_db_clear() {
                self.recycle_storage(
                    vec![address],
                    debug_record.as_deref_mut(),
                )?;
            }

            if !account.removed_without_update() {
                accounts_to_notify.push(account.as_account());
                account.commit(
                    &mut self.db,
                    &address,
                    debug_record.as_deref_mut(),
                )?;
            }
        }
        Ok(accounts_to_notify)
    }

    fn recycle_storage(
        &mut self, killed_addresses: Vec<AddressWithSpace>,
        mut debug_record: Option<&mut ComputeEpochDebugRecord>,
    ) -> DbResult<()> {
        // TODO: Think about kill_dust and collateral refund.
        for address in &killed_addresses {
            self.db
                .clear_account_storage(address, debug_record.as_deref_mut())?;
            self.db
                .clear_account_code(address, debug_record.as_deref_mut())?;
            self.db.delete(
                StorageKey::new_account_key(&address.address)
                    .with_space(address.space),
                debug_record.as_deref_mut(),
            )?;
            self.db.delete(
                StorageKey::new_deposit_list_key(&address.address)
                    .with_space(address.space),
                debug_record.as_deref_mut(),
            )?;
            self.db.delete(
                StorageKey::new_vote_list_key(&address.address)
                    .with_space(address.space),
                debug_record.as_deref_mut(),
            )?;
        }
        Ok(())
    }
}

impl State<'_> {
    pub fn commit_cache(&mut self, retain_transient_storage: bool) {
        assert!(self.no_checkpoint());
        for (addr, mut account) in self.cache.get_mut().drain() {
            if let AccountEntry::Cached(ref mut acc, dirty) = account.entry {
                acc.commit_cache(retain_transient_storage, dirty);
            }
            self.committed_cache.insert(addr, account.entry);
        }
    }
}
