#[allow(unused_imports)]
use crate::alloc_prelude::*;
use super::{Core, Handle};

impl Handle {
    pub(super) fn trace_core(&self, core: Box<Core>) -> Box<Core> {
        core
    }
}
