/*
 * SPDX-FileCopyrightText: © Hypermode Inc. <hello@hypermode.com>
 * SPDX-License-Identifier: Apache-2.0
 */

use clap::{Args, Subcommand};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::time::sleep;
use rand::Rng;

use melisdb::{Options, KV, Error, Transaction, TransactionManager};

#[derive(Subcommand)]
pub enum BankCommands {
    /// Execute bank test on MelisDB
    Test {
        #[command(flatten)]
        test_args: BankTestArgs,
    },
    /// Dissect the bank output to find the first transaction which causes failure
    Dissect {
        #[command(flatten)]
        dissect_args: BankDissectArgs,
    },
}

#[derive(Args)]
pub struct BankTestArgs {
    /// Number of concurrent transactions to run
    #[arg(long, short = 'c', default_value = "16")]
    conc: usize,

    /// How long to run the test
    #[arg(long, default_value = "3m")]
    duration: String,

    /// If true, the test will send transactions to another badger instance via the stream interface
    #[arg(long, short = 's')]
    check_stream: bool,

    /// If true, the test will send transactions to another badger instance via the subscriber interface
    #[arg(long, short = 'w')]
    check_subscriber: bool,

    /// If true, the test will print all the executed bank transfers to standard output
    #[arg(long, short = 'v')]
    verbose: bool,

    /// If set, badger will encrypt all the data stored on the disk
    #[arg(long, short = 'e')]
    encryption_key: Option<String>,
}

#[derive(Args)]
pub struct BankDissectArgs {
    /// Starting from the violation txn, how many previous versions to retrieve
    #[arg(long, short = 'p', default_value = "12")]
    previous: usize,

    /// If set, DB will be opened using the provided decryption key
    #[arg(long)]
    decryption_key: Option<String>,
}

const KEY_PREFIX: &str = "account:";
const INITIAL_BALANCE: u64 = 100;

pub async fn handle_bank_command(
    command: BankCommands,
    accounts: usize,
    sst_dir: PathBuf,
    vlog_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        BankCommands::Test { test_args } => {
            run_bank_test(test_args, accounts, sst_dir, vlog_dir).await?;
        }
        BankCommands::Dissect { dissect_args } => {
            run_bank_dissect(dissect_args, sst_dir, vlog_dir).await?;
        }
    }
    Ok(())
}

fn account_key(account: usize) -> Vec<u8> {
    format!("{}{}", KEY_PREFIX, account).into_bytes()
}

fn balance_to_bytes(balance: u64) -> Vec<u8> {
    balance.to_string().into_bytes()
}

fn bytes_to_balance(bytes: &[u8]) -> Result<u64, std::num::ParseIntError> {
    std::str::from_utf8(bytes)
        .unwrap_or("0")
        .parse::<u64>()
}

async fn get_balance(kv: &KV, account: usize) -> Result<u64, Error> {
    let key = account_key(account);
    let value = kv.get(&key).await?;
    Ok(bytes_to_balance(&value).unwrap_or(0))
}

async fn put_balance(kv: &KV, account: usize, balance: u64) -> Result<(), Error> {
    let key = account_key(account);
    let value = balance_to_bytes(balance);
    kv.set(key, value, 0).await
}

fn min(a: u64, b: u64) -> u64 {
    if a < b { a } else { b }
}

async fn get_balance_txn(txn: &Transaction, account: usize) -> Result<u64, BankError> {
    let key = account_key(account);
    match txn.get(&key).await {
        Ok(Some(value)) => {
            if value.is_empty() {
                Ok(INITIAL_BALANCE)
            } else {
                Ok(bytes_to_balance(&value).unwrap_or(0))
            }
        }
        Ok(None) => Ok(INITIAL_BALANCE),
        Err(_) => Err(BankError::InsufficientBalance),
    }
}

async fn put_balance_txn(txn: &mut Transaction, account: usize, balance: u64) -> Result<(), BankError> {
    let key = account_key(account);
    let value = balance_to_bytes(balance);
    txn.set(&key, &value).await.map_err(|_| BankError::InsufficientBalance)
}

#[derive(Debug, thiserror::Error)]
enum BankError {
    #[error("Transaction abandoned due to insufficient balance")]
    InsufficientBalance,
    #[error("Test failed due to balance mismatch")]
    BalanceMismatch,
}

// Global lock to ensure atomicity of bank operations
// This is a temporary solution until proper database transactions are implemented
static BANK_LOCK: Mutex<()> = Mutex::new(());

async fn move_money(kv: &KV, from: usize, to: usize) -> Result<(), BankError> {
    // Create a transaction manager
    let txn_mgr = TransactionManager::new(Arc::new(kv.clone()));
    
    // Use a retry mechanism with proper transactions
    for _ in 0..10 {
        // Create a new read-write transaction
        let mut txn = txn_mgr.new_read_write_txn();
        
        // Get current balances within the transaction
        let from_balance = get_balance_txn(&txn, from).await.map_err(|_| BankError::InsufficientBalance)?;
        let to_balance = get_balance_txn(&txn, to).await.map_err(|_| BankError::InsufficientBalance)?;

        let floor = min(from_balance, to_balance);
        if floor < 5 {
            return Err(BankError::InsufficientBalance);
        }

        // Move $5 from 'from' to 'to'
        let new_from_balance = from_balance - 5;
        let new_to_balance = to_balance + 5;

        // Update both balances within the transaction
        put_balance_txn(&mut txn, from, new_from_balance).await.map_err(|_| BankError::InsufficientBalance)?;
        put_balance_txn(&mut txn, to, new_to_balance).await.map_err(|_| BankError::InsufficientBalance)?;

        // Commit the transaction
        match txn.commit().await {
            Ok(()) => return Ok(()),
            Err(_) => {
                // Transaction failed, retry after a small delay
                sleep(Duration::from_micros(1)).await;
            }
        }
    }

    Err(BankError::InsufficientBalance)
}

async fn seek_total(kv: &KV, num_accounts: usize) -> Result<u64, BankError> {
    let expected_total = (num_accounts as u64) * INITIAL_BALANCE;
    let mut total = 0;

    for i in 0..num_accounts {
        let balance = get_balance(kv, i).await.map_err(|_| BankError::BalanceMismatch)?;
        total += balance;
    }

    if total != expected_total {
        tracing::error!(
            "Balance did NOT match up. Expected: {}. Received: {}",
            expected_total,
            total
        );
        return Err(BankError::BalanceMismatch);
    }

    Ok(total)
}

async fn run_bank_test(
    args: BankTestArgs,
    accounts: usize,
    sst_dir: PathBuf,
    vlog_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let _rng = rand::thread_rng();

    // Parse duration
    let duration = parse_duration(&args.duration)?;

    // Open database
    let mut opts = Options::default();
    *opts.dir = sst_dir.to_string_lossy().to_string();
    *opts.value_dir = vlog_dir.to_string_lossy().to_string();
    
    if let Some(key) = &args.encryption_key {
        tracing::info!("Using encryption key: {}", key);
        // TODO: Add encryption support when available in Options
    }

    tracing::info!("Opening DB with options: {:?}", opts);
    let kv = KV::open(opts).await?;

    // Initialize accounts
    tracing::info!("Initializing {} accounts with balance {}", accounts, INITIAL_BALANCE);
    for i in 0..accounts {
        put_balance(&kv, i, INITIAL_BALANCE).await?;
    }

    tracing::info!("Bank initialization OK. Commencing test.");
    tracing::info!("Running with {} accounts, and {} goroutines.", accounts, args.conc);
    tracing::info!("Using keyPrefix: {}", KEY_PREFIX);

    let end_time = SystemTime::now() + duration;
    let total_transactions = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let reads = Arc::new(AtomicU64::new(0));
    let stop_all = Arc::new(AtomicU64::new(0));

    // Spawn concurrent transaction workers
    let mut handles = Vec::new();

    for _ in 0..args.conc {
        let kv_clone = kv.clone();
        let total_clone = total_transactions.clone();
        let errors_clone = errors.clone();
        let stop_all_clone = stop_all.clone();
        let num_accounts = accounts;
        let verbose = args.verbose;

        let handle = tokio::spawn(async move {
            loop {
                if stop_all_clone.load(Ordering::Relaxed) > 0 {
                    break;
                }
                if SystemTime::now() > end_time {
                    break;
                }

                let from = rand::thread_rng().gen_range(0..num_accounts);
                let to = rand::thread_rng().gen_range(0..num_accounts);
                if from == to {
                    continue;
                }

                // Add timeout to prevent hanging
                match tokio::time::timeout(Duration::from_millis(100), move_money(&kv_clone, from, to)).await {
                    Ok(Ok(())) => {
                        total_clone.fetch_add(1, Ordering::Relaxed);
                        if verbose {
                            tracing::info!("Moved $5. {} -> {}", from, to);
                        }
                    }
                    Ok(Err(_)) => {
                        errors_clone.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        // Timeout occurred
                        errors_clone.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!("Transaction timeout for {} -> {}", from, to);
                    }
                }

                // Small delay to prevent overwhelming the system
                sleep(Duration::from_micros(10)).await;
            }
        });
        handles.push(handle);
    }

    // Spawn read-only worker to verify total
    let kv_clone = kv.clone();
    let reads_clone = reads.clone();
    let stop_all_clone = stop_all.clone();
    let num_accounts = accounts;

    let read_handle = tokio::spawn(async move {
        loop {
            if stop_all_clone.load(Ordering::Relaxed) > 0 {
                break;
            }
            if SystemTime::now() > end_time {
                break;
            }

            match tokio::time::timeout(Duration::from_millis(100), seek_total(&kv_clone, num_accounts)).await {
                Ok(Ok(_)) => {
                    reads_clone.fetch_add(1, Ordering::Relaxed);
                }
                Ok(Err(BankError::BalanceMismatch)) => {
                    tracing::error!("Error while calculating total: Balance mismatch");
                    stop_all_clone.store(1, Ordering::Relaxed);
                    break;
                }
                Ok(Err(e)) => {
                    tracing::error!("Error while calculating total: {:?}", e);
                }
                Err(_) => {
                    // Timeout occurred
                    tracing::warn!("Seek total timeout");
                }
            }

            sleep(Duration::from_micros(10)).await;
        }
    });

    // Wait for all workers to complete
    for handle in handles {
        let _ = handle.await;
    }
    let _ = read_handle.await;

    let final_total = total_transactions.load(Ordering::Relaxed);
    let final_errors = errors.load(Ordering::Relaxed);
    let final_reads = reads.load(Ordering::Relaxed);

    tracing::info!("Test completed. Total transactions: {}, Errors: {}, Reads: {}", 
                   final_total, final_errors, final_reads);

    if stop_all.load(Ordering::Relaxed) == 0 {
        tracing::info!("Test OK");
        Ok(())
    } else {
        tracing::error!("Test FAILED");
        Err("Test FAILED".into())
    }
}

async fn run_bank_dissect(
    _args: BankDissectArgs,
    _sst_dir: PathBuf,
    _vlog_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    // TODO: Implement bank dissect functionality
    tracing::warn!("Bank dissect functionality not yet implemented");
    Ok(())
}

fn parse_duration(duration_str: &str) -> Result<Duration, Box<dyn std::error::Error>> {
    let duration_str = duration_str.trim();
    let (num_str, unit) = if duration_str.ends_with("s") {
        (&duration_str[..duration_str.len()-1], "s")
    } else if duration_str.ends_with("m") {
        (&duration_str[..duration_str.len()-1], "m")
    } else if duration_str.ends_with("h") {
        (&duration_str[..duration_str.len()-1], "h")
    } else {
        return Err("Invalid duration format. Use format like '3m', '30s', '1h'".into());
    };

    let num: u64 = num_str.parse()?;
    
    match unit {
        "s" => Ok(Duration::from_secs(num)),
        "m" => Ok(Duration::from_secs(num * 60)),
        "h" => Ok(Duration::from_secs(num * 3600)),
        _ => Err("Invalid duration unit. Use 's', 'm', or 'h'".into()),
    }
}
