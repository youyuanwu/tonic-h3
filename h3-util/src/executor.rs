use futures::future::BoxFuture;
use hyper_util::rt::TokioExecutor;
use std::{future::Future, sync::Arc};

// copied from https://github.com/hyperium/tonic/blob/master/tonic/src/transport/channel/service/executor.rs#L8

pub(crate) use hyper::rt::Executor;

#[derive(Clone)]
pub struct SharedExec {
    inner: Arc<dyn Executor<BoxFuture<'static, ()>> + Send + Sync + 'static>,
}

impl SharedExec {
    pub fn new<E>(exec: E) -> Self
    where
        E: Executor<BoxFuture<'static, ()>> + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(exec),
        }
    }

    pub fn tokio() -> Self {
        Self::new(TokioExecutor::new())
    }
}

impl<F> Executor<F> for SharedExec
where
    F: Future<Output = ()> + Send + 'static,
{
    fn execute(&self, fut: F) {
        self.inner.execute(Box::pin(fut))
    }
}
