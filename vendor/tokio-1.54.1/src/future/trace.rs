#[allow(unused_imports)]
use crate::alloc_prelude::*;
use ::core::future::Future;

pub(crate) trait InstrumentedFuture: Future {
    fn id(&self) -> Option<tracing::Id>;
}

impl<F: Future> InstrumentedFuture for tracing::instrument::Instrumented<F> {
    fn id(&self) -> Option<tracing::Id> {
        self.span().id()
    }
}
