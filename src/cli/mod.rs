//! Command-line interface modules for the Badger database tool.
//!
//! This module contains all CLI subcommands and their implementations.

pub mod bank;
pub mod info;
pub mod stream;

// Re-export the main command structures for easy access
pub use bank::{BankCommands, BankTestArgs, BankDissectArgs};
// pub use info::InfoArgs;
pub use stream::StreamArgs;
