// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

#![allow(clippy::mut_from_ref)]

#[macro_use]
mod maybe_in_place_byte_array_macro;

pub mod children_table;
pub mod compressed_path;
pub mod maybe_in_place_byte_array;
pub mod merkle;
pub mod trie_node;
pub mod trie_proof;
pub mod walk;

mod utils;

#[cfg(test)]
mod tests;

pub use self::{
    children_table::*,
    compressed_path::{
        CompressedPathRaw, CompressedPathRef, CompressedPathTrait,
    },
    trie_node::{TrieNodeTrait, VanillaTrieNode},
    trie_proof::{TrieProof, TrieProofNode},
    utils::WrappedCreateFrom,
};
