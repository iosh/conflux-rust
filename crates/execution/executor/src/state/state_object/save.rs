use std::{collections::HashMap, fmt};

use crate::state::{global_stat::GlobalStat, overlay_account::AccountEntry};
use cfx_types::AddressWithSpace;

use super::State;

/// In-memory state over an unchanged backing database. This does not retain
/// transaction checkpoints, access lists, or transient storage.
pub struct SavedState {
    committed_cache: HashMap<AddressWithSpace, AccountEntry>,
    global_stat: GlobalStat,
}

impl fmt::Debug for SavedState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SavedState")
            .field("accounts", &self.committed_cache.len())
            .finish_non_exhaustive()
    }
}

impl State {
    /// Copy the currently visible in-memory state without changing execution.
    /// Unlike `save`, this can be called while checkpoints are active. The
    /// result can be restored into an independent State backed by the same
    /// unchanged database, but is not a snapshot of the backing database.
    pub fn snapshot(&self) -> SavedState {
        let cache = self.cache.read();
        let entries = cache
            .iter()
            .map(|(address, entry)| (address, &entry.entry))
            .chain(
                self.committed_cache
                    .iter()
                    .filter(|(address, _)| !cache.contains_key(address)),
            );
        let committed_cache = entries
            .map(|(address, entry)| {
                let mut entry = entry.clone_account();
                if let AccountEntry::Cached(account, dirty) = &mut entry {
                    // Only the independent copy is normalized for a fresh
                    // transaction; the source checkpoint stack is untouched.
                    account.commit_cache(false, *dirty);
                }
                (*address, entry)
            })
            .collect();
        SavedState {
            committed_cache,
            global_stat: self.global_stat,
        }
    }

    pub fn save(&mut self) -> SavedState {
        self.commit_cache(false);
        let committed_cache = self
            .committed_cache
            .iter()
            .map(|(k, v)| (*k, v.clone_account()))
            .collect();
        SavedState {
            committed_cache,
            global_stat: self.global_stat.clone(),
        }
    }

    pub fn restore(&mut self, saved: SavedState) {
        assert!(self.no_checkpoint());
        self.cache = Default::default();
        self.committed_cache = saved.committed_cache;
        self.global_stat = saved.global_stat;
    }
}
