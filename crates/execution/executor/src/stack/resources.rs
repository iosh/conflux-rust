use crate::{
    executive_observer::TracerTrait, stack::CallStackInfo, state::State,
};

/// The global resources and utilities shared across all frames.
pub struct RuntimeRes<'a, 'db> {
    /// The ledger state including information such as the balance of each
    /// account.
    pub state: &'a mut State<'db>,

    /// Metadata about the frame call stack.
    pub callstack: &'a mut CallStackInfo,

    /// A tool for recording information about the execution as it proceeds.
    /// The data captured by the tracer is not used for consensus-critical
    /// operations.
    pub tracer: &'a mut dyn TracerTrait,
}

#[cfg(test)]
pub mod runtime_res_test {
    use super::RuntimeRes;
    use crate::stack::CallStackInfo;

    use super::State;

    pub struct OwnedRuntimeRes<'a, 'db> {
        state: &'a mut State<'db>,
        callstack: CallStackInfo,
        tracer: (),
    }

    impl<'a, 'db> From<&'a mut State<'db>> for OwnedRuntimeRes<'a, 'db> {
        fn from(state: &'a mut State<'db>) -> Self {
            OwnedRuntimeRes {
                state,
                callstack: CallStackInfo::new(),
                tracer: (),
            }
        }
    }

    impl<'a, 'db> OwnedRuntimeRes<'a, 'db> {
        pub fn as_res<'b>(&'b mut self) -> RuntimeRes<'b, 'db> {
            RuntimeRes {
                state: &mut self.state,
                callstack: &mut self.callstack,
                tracer: &mut self.tracer,
            }
        }
    }
}
