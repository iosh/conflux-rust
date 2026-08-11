// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

pub trait AccessMode {
    const READ_ONLY: bool;
}

pub struct Read;
pub struct Write;

impl AccessMode for Read {
    const READ_ONLY: bool = true;
}

impl AccessMode for Write {
    const READ_ONLY: bool = false;
}
