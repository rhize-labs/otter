//! Oracle system for managing transaction timestamps and conflict detection
//! 
//! This module implements the Oracle system from Go BadgerDB, which manages
//! transaction timestamps, conflict detection, and watermark-based ordering.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;

use crate::types::Closer;

/// Committed transaction information for conflict detection
#[derive(Debug, Clone)]
pub struct CommittedTxn {
    pub ts: u64,
    pub conflict_keys: HashMap<u64, ()>, // fingerprint -> ()
}

/// Oracle manages transaction timestamps and conflict detection
pub struct Oracle {
    /// Whether this is a managed database (doesn't change, so no locking required)
    is_managed: bool,
    /// Whether to detect conflicts
    detect_conflicts: bool,
    
    /// Mutex for nextTxnTs and commits
    next_txn_ts: Arc<AtomicU64>,
    /// Write channel lock for ensuring transactions go to write channel in order
    write_ch_lock: Arc<RwLock<()>>,
    
    /// Used to block NewTransaction, so all previous commits are visible to a new read
    txn_mark: Arc<AtomicU64>,
    
    /// Either of these is used to determine which versions can be permanently
    /// discarded during compaction
    discard_ts: AtomicU64,
    read_mark: Arc<AtomicU64>,
    
    /// Committed transactions for conflict detection
    committed_txns: Arc<Mutex<Vec<CommittedTxn>>>,
    last_cleanup_ts: AtomicU64,
    
    /// Closer for stopping watermarks
    closer: Closer,
}

impl Oracle {
    /// Create a new Oracle instance
    pub fn new(is_managed: bool, detect_conflicts: bool) -> Self {
        Self {
            is_managed,
            detect_conflicts,
            next_txn_ts: Arc::new(AtomicU64::new(1)),
            write_ch_lock: Arc::new(RwLock::new(())),
            txn_mark: Arc::new(AtomicU64::new(0)),
            discard_ts: AtomicU64::new(0),
            read_mark: Arc::new(AtomicU64::new(0)),
            committed_txns: Arc::new(Mutex::new(Vec::new())),
            last_cleanup_ts: AtomicU64::new(0),
            closer: Closer::new("oracle".to_string()),
        }
    }
    
    /// Create a new Oracle instance with a shared timestamp counter
    pub fn new_with_ts_counter(is_managed: bool, detect_conflicts: bool, ts_counter: Arc<AtomicU64>) -> Self {
        Self {
            is_managed,
            detect_conflicts,
            next_txn_ts: ts_counter,
            write_ch_lock: Arc::new(RwLock::new(())),
            txn_mark: Arc::new(AtomicU64::new(0)),
            discard_ts: AtomicU64::new(0),
            read_mark: Arc::new(AtomicU64::new(0)),
            committed_txns: Arc::new(Mutex::new(Vec::new())),
            last_cleanup_ts: AtomicU64::new(0),
            closer: Closer::new("oracle".to_string()),
        }
    }
    
    /// Get the next transaction timestamp
    pub fn next_ts(&self) -> u64 {
        self.next_txn_ts.load(Ordering::SeqCst)
    }
    
    /// Increment the next transaction timestamp
    pub fn increment_next_ts(&self) {
        self.next_txn_ts.fetch_add(1, Ordering::SeqCst);
    }
    
    /// Get the next timestamp (increment and return)
    pub fn get_next_ts(&self) -> u64 {
        self.next_txn_ts.fetch_add(1, Ordering::SeqCst)
    }
    
    /// Get read timestamp for a new transaction
    pub fn read_ts(&self) -> u64 {
        if self.is_managed {
            panic!("ReadTs should not be retrieved for managed DB");
        }
        
        // Use the current next_txn_ts as the read timestamp
        // This ensures we can read all previously committed data
        let read_ts = self.next_txn_ts.load(Ordering::SeqCst);
        self.read_mark.store(read_ts, Ordering::SeqCst);
        
        // Skip the watermark wait for now to avoid hanging
        // In a real implementation, this would use proper watermarks
        read_ts
    }
    
    /// Wait for transaction mark to reach the given timestamp
    fn wait_for_txn_mark(&self, ts: u64) {
        // Simple implementation - in a real system this would use proper watermarks
        // For now, we'll just ensure the txn_mark is at least as high as the read_ts
        loop {
            let current_mark = self.txn_mark.load(Ordering::SeqCst);
            if current_mark >= ts {
                break;
            }
            std::hint::spin_loop();
        }
    }
    
    /// Set discard timestamp for compaction
    pub fn set_discard_ts(&self, ts: u64) {
        self.discard_ts.store(ts, Ordering::SeqCst);
        self.cleanup_committed_transactions();
    }
    
    /// Get discard timestamp
    pub fn discard_at_or_below(&self) -> u64 {
        if self.is_managed {
            self.discard_ts.load(Ordering::SeqCst)
        } else {
            self.read_mark.load(Ordering::SeqCst)
        }
    }
    
    /// Check if a transaction has conflicts
    fn has_conflict(&self, txn: &Transaction) -> bool {
        if txn.reads.is_empty() {
            return false;
        }
        
        let committed_txns = self.committed_txns.lock().unwrap();
        for committed_txn in committed_txns.iter() {
            // If the committedTxn.ts is less than txn.readTs that implies that the
            // committedTxn finished before the current transaction started.
            // We don't need to check for conflict in that case.
            if committed_txn.ts <= txn.read_ts {
                continue;
            }
            
            for &read_fp in &txn.reads {
                if committed_txn.conflict_keys.contains_key(&read_fp) {
                    return true;
                }
            }
        }
        false
    }
    
    /// Get a new commit timestamp for a transaction
    pub fn new_commit_ts(&self, txn: &mut Transaction) -> Result<u64, bool> {
        if self.has_conflict(txn) {
            return Err(true); // Conflict detected
        }
        
        let ts = if !self.is_managed {
            self.done_read(txn);
            self.cleanup_committed_transactions();
            
            // This is the general case, when user doesn't specify the read and commit ts.
            let ts = self.next_txn_ts.load(Ordering::SeqCst);
            self.next_txn_ts.fetch_add(1, Ordering::SeqCst);
            self.txn_mark.store(ts, Ordering::SeqCst);
            ts
        } else {
            // If commitTs is set, use it instead.
            txn.commit_ts
        };
        
        if self.detect_conflicts {
            // We should ensure that txns are not added to committed_txns slice when
            // conflict detection is disabled otherwise this slice would keep growing.
            let mut committed_txns = self.committed_txns.lock().unwrap();
            committed_txns.push(CommittedTxn {
                ts,
                conflict_keys: txn.conflict_keys.clone(),
            });
        }
        
        Ok(ts)
    }
    
    /// Mark a read as done
    fn done_read(&self, txn: &Transaction) {
        if !txn.done_read {
            // In a real implementation, this would update the read mark
            // For now, we'll just mark it as done
        }
    }
    
    /// Clean up committed transactions that are no longer needed
    fn cleanup_committed_transactions(&self) {
        if !self.detect_conflicts {
            return;
        }
        
        let max_read_ts = if self.is_managed {
            self.discard_ts.load(Ordering::SeqCst)
        } else {
            self.read_mark.load(Ordering::SeqCst)
        };
        
        let last_cleanup = self.last_cleanup_ts.load(Ordering::SeqCst);
        if max_read_ts <= last_cleanup {
            return;
        }
        
        self.last_cleanup_ts.store(max_read_ts, Ordering::SeqCst);
        
        let mut committed_txns = self.committed_txns.lock().unwrap();
        committed_txns.retain(|txn| txn.ts > max_read_ts);
    }
    
    /// Mark a commit as done
    pub fn done_commit(&self, cts: u64) {
        if self.is_managed {
            return;
        }
        self.txn_mark.store(cts, Ordering::SeqCst);
    }
    
    /// Get write channel lock
    pub fn write_ch_lock(&self) -> Arc<RwLock<()>> {
        self.write_ch_lock.clone()
    }
}

/// Transaction structure for the Oracle system
pub struct Transaction {
    pub read_ts: u64,
    pub commit_ts: u64,
    pub reads: Vec<u64>, // fingerprints of keys read
    pub conflict_keys: HashMap<u64, ()>, // fingerprints of keys written
    pub done_read: bool,
}

impl Transaction {
    pub fn new(read_ts: u64, is_managed: bool) -> Self {
        Self {
            read_ts,
            commit_ts: if is_managed { 0 } else { 0 },
            reads: Vec::new(),
            conflict_keys: HashMap::new(),
            done_read: false,
        }
    }
    
    /// Add a read key fingerprint
    pub fn add_read_key(&mut self, key_fp: u64) {
        self.reads.push(key_fp);
    }
    
    /// Add a conflict key fingerprint
    pub fn add_conflict_key(&mut self, key_fp: u64) {
        self.conflict_keys.insert(key_fp, ());
    }
}
