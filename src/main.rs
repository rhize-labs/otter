/*
 * SPDX-FileCopyrightText: © Hypermode Inc. <hello@hypermode.com>
 * SPDX-License-Identifier: Apache-2.0
 */

use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod cli;

#[derive(Parser)]
#[command(name = "otter")]
#[command(about = "Tools to manage OtterDB database.")]
#[command(version)]
struct Cli {
    /// Directory where the LSM tree files are located (required)
    #[arg(long, short = 'd', global = true)]
    dir: Option<PathBuf>,

    /// Directory where the value log files are located, if different from --dir
    #[arg(long, global = true)]
    vlog_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run bank test on OtterDB
    Bank {
        /// Number of accounts in the bank
        #[arg(long, short = 'a', default_value = "10000")]
        accounts: usize,
        
        #[command(subcommand)]
        bank_command: cli::bank::BankCommands,
    },
    /// Health info about OtterDB database
    Info {
        #[command(flatten)]
        info_args: cli::info::InfoArgs,
    },
    /// Stream DB into another DB with different options
    Stream {
        #[command(flatten)]
        stream_args: cli::stream::StreamArgs,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    let env = tracing_subscriber::EnvFilter::from_default_env();
    tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(env)
        .try_init()
        .unwrap_or_else(|_| {
            // Fallback to basic logging if tracing fails
            env_logger::init();
        });

    let cli = Cli::parse();

    // Get directories (--dir is required)
    let sst_dir = cli.dir.ok_or("--dir is required")?;
    let vlog_dir = cli.vlog_dir.unwrap_or_else(|| sst_dir.clone());

    match cli.command {
        Commands::Bank { accounts, bank_command } => {
            cli::bank::handle_bank_command(bank_command, accounts, sst_dir, vlog_dir).await?;
        }
        Commands::Info { info_args } => {
            cli::info::handle_info_command(info_args, sst_dir, vlog_dir).await?;
        }
        Commands::Stream { stream_args } => {
            cli::stream::handle_stream_command(stream_args, sst_dir, vlog_dir).await?;
        }
    }

    Ok(())
}
