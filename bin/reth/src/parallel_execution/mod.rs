//! `reth parallel-execution` command. Collection of various debugging routines.
use clap::{Parser, Subcommand};

use crate::runner::CliContext;

mod generate;
mod trace_diff;

/// `reth parallel-execution` command
#[derive(Debug, Parser)]
pub struct Command {
    #[clap(subcommand)]
    command: Subcommands,
}

/// `reth parallel-execution` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands {
    /// The command for generating historical execution DAGs.
    Generate(generate::Command),
    /// The command for comparing traces between normal and parallel execution.
    TraceDiff(trace_diff::Command),
}

impl Command {
    /// Execute `parallel-execution` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        match self.command {
            Subcommands::Generate(command) => command.execute(ctx).await,
            Subcommands::TraceDiff(command) => command.execute(ctx).await,
        }
    }
}
