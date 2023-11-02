//! EVM execution node arguments

use clap::Args;
use eyre::Context;
use reth_primitives::ChainSpec;
use reth_provider::{AsyncExecutorFactory, PrunableAsyncBlockExecutor};
use reth_revm::{
    parallel::{factory::ParallelExecutorFactory, queue::BlockQueueStore},
    EVMProcessorFactory,
};
use std::{path::PathBuf, sync::Arc};

/// Parameters for EVM execution
#[derive(Debug, Args, PartialEq, Default)]
#[command(next_help_heading = "Execution")]
pub struct ExecutionArgs {
    /// Run historical execution in parallel.
    #[arg(long = "execution.parallel", default_value_t = false)]
    pub parallel: bool,

    /// Path to the block queues for parallel execution.
    #[arg(long = "execution.parallel-queue-store", required_if_eq("parallel", "true"))]
    pub queue_store: Option<PathBuf>,
}

impl ExecutionArgs {
    /// Returns executor factory to be used in historical sync.
    pub fn pipeline_executor_factory(
        &self,
        chain_spec: Arc<ChainSpec>,
    ) -> eyre::Result<EitherExecutorFactory<EVMProcessorFactory, ParallelExecutorFactory>> {
        let factory = if self.parallel {
            let queue_store_content =
                std::fs::read_to_string(self.queue_store.as_ref().expect("is set"))
                    .wrap_err("failed to read parallel queue store")?;
            let queues = serde_json::from_str(&queue_store_content)
                .wrap_err("failed to deserialize queue store")?;
            EitherExecutorFactory::Right(ParallelExecutorFactory::new(
                chain_spec,
                Arc::new(BlockQueueStore::new(queues)),
            ))
        } else {
            EitherExecutorFactory::Left(EVMProcessorFactory::new(chain_spec))
        };
        Ok(factory)
    }
}

/// A type that represents one of two possible executor factories.
#[derive(Debug, Clone)]
pub enum EitherExecutorFactory<A, B> {
    /// The first factory variant
    Left(A),
    /// The second factory variant
    Right(B),
}

impl<A, B> AsyncExecutorFactory for EitherExecutorFactory<A, B>
where
    A: AsyncExecutorFactory,
    B: AsyncExecutorFactory,
{
    fn chain_spec(&self) -> &ChainSpec {
        match self {
            EitherExecutorFactory::Left(a) => a.chain_spec(),
            EitherExecutorFactory::Right(b) => b.chain_spec(),
        }
    }

    fn with_state<'a, SP: reth_provider::StateProvider + 'a>(
        &'a self,
        sp: SP,
    ) -> Box<dyn PrunableAsyncBlockExecutor + 'a> {
        match self {
            EitherExecutorFactory::Left(a) => a.with_state::<'a, SP>(sp),
            EitherExecutorFactory::Right(b) => b.with_state::<'a, SP>(sp),
        }
    }
}
