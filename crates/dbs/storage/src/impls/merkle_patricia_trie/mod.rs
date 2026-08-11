// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

pub mod mpt_cursor;
pub mod mpt_merger;
pub mod simple_mpt;

pub use cfx_mpt::{children_table, merkle, trie_node, trie_proof};
pub(super) use cfx_mpt::{compressed_path, maybe_in_place_byte_array, walk};

pub use self::mpt_merger::MptMerger;
pub use cfx_mpt::{
    children_table::*, CompressedPathRaw, CompressedPathRef,
    CompressedPathTrait, TrieNodeTrait, TrieProof, VanillaTrieNode,
};

pub use cfx_storage_types::MptKeyValue;

/// Classes implement KVInserter is used to store key-values in MPT iteration.
pub trait KVInserter<Value> {
    fn push(&mut self, v: Value) -> Result<()>;
}

impl<Value> KVInserter<Value> for Vec<Value> {
    fn push(&mut self, v: Value) -> Result<()> { Ok((*self).push(v)) }
}

use super::errors::Result;
