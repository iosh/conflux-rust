// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

#[macro_use]
pub mod deref_plus_impl_or_borrow_self;
#[macro_use]
pub mod tuple;
pub mod guarded_value;
pub mod wrap;

pub use cfx_mpt::WrappedCreateFrom;
pub use cfx_storage_types::{access_mode, to_key_prefix_iter_upper_bound};

pub trait UnsafeCellExtension<T: Sized> {
    fn get_ref(&self) -> &T;
    fn get_mut(&mut self) -> &mut T;
    unsafe fn get_as_mut(&self) -> &mut T;
}

impl<T: Sized> UnsafeCellExtension<T> for UnsafeCell<T> {
    fn get_ref(&self) -> &T { unsafe { &*self.get() } }

    fn get_mut(&mut self) -> &mut T { unsafe { &mut *self.get() } }

    unsafe fn get_as_mut(&self) -> &mut T { &mut *self.get() }
}

/// Only used by storage benchmark due to incompatibility of rlp crate version.
pub trait StateRootWithAuxInfoToFromRlpBytes {
    fn to_rlp_bytes(&self) -> Vec<u8>;
    fn from_rlp_bytes(bytes: &[u8]) -> Result<StateRootWithAuxInfo>;
}

/// Only used by storage benchmark due to incompatibility of rlp crate version.
impl StateRootWithAuxInfoToFromRlpBytes for StateRootWithAuxInfo {
    fn to_rlp_bytes(&self) -> Vec<u8> { self.rlp_bytes().to_vec() }

    fn from_rlp_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self::decode(&Rlp::new(bytes))?)
    }
}

use crate::Result;
use cfx_internal_common::StateRootWithAuxInfo;
use rlp::{Decodable, Encodable, Rlp};
use std::cell::UnsafeCell;
