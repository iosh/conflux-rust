use cfx_types::{AddressWithSpace, U256};
use impl_tools::autoimpl;
use impl_trait_for_tuples::impl_for_tuples;

#[impl_for_tuples(4)]
#[autoimpl(for<T: trait + ?Sized> &mut T)]
pub trait StorageTracer {
    fn trace_storage_write(
        &mut self, _address: AddressWithSpace, _key: &[u8], _value: U256,
    ) {
    }
}
