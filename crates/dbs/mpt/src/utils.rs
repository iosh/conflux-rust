// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

use std::cell::UnsafeCell;

/// The purpose of this trait is to create a new value of a passed object,
/// when the passed object is the value, simply move the value;
/// when the passed object is the reference, create the new value by clone.
/// Other extension is possible.
///
/// The trait is used by ChildrenTable and slab.
pub trait WrappedCreateFrom<FromType, ToType> {
    fn take(x: FromType) -> ToType;

    /// Unoptimized default implementation.
    fn take_from(dest: &mut ToType, x: FromType) { *dest = Self::take(x); }
}

impl<T> WrappedCreateFrom<UnsafeCell<T>, UnsafeCell<T>> for UnsafeCell<T> {
    fn take(val: UnsafeCell<T>) -> UnsafeCell<T> { val }
}

/*
/// This is correct but we don't use this implementation.
impl<T> WrappedCreateFrom<T, UnsafeCell<T>> for UnsafeCell<T> {
    fn take(val: T) -> UnsafeCell<T> { UnsafeCell::new(val) }
}
*/

impl<'x, T: Clone> WrappedCreateFrom<&'x T, UnsafeCell<T>> for UnsafeCell<T> {
    fn take(val: &'x T) -> UnsafeCell<T> { UnsafeCell::new(val.clone()) }

    fn take_from(dest: &mut UnsafeCell<T>, x: &'x T) {
        dest.get_mut().clone_from(x);
    }
}
