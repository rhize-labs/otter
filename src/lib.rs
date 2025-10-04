// Removed unstable features for stable Rust compatibility


/// OtterDB is an embedded key-value database.
///
/// OtterDB is a library written in Rust that implements a BadgerDB-compatible database.
/// OtterDB implements all features of the original BadgerDB.
use std::mem::align_of;

mod event;
mod iterator;
pub mod kv;
mod level_handler;
mod log_file;
mod manifest;
mod oracle;
mod options;
mod skl;
mod table;
mod transaction;
mod types;
mod value_log;
#[cfg(test)]
mod value_log_tests;
mod y;

mod compaction;
// #[cfg(test)]
// mod kv_test;
#[cfg(test)]
mod kv_test;
mod levels;
mod pb;
mod st_manager;
#[cfg(test)]
mod test_util;
mod backup;

pub use iterator::*;
pub use kv::*;
pub use options::*;
pub use skl::*;
pub use st_manager::*;
pub use transaction::*;
pub use value_log::Entry;
pub use y::*;

#[allow(dead_code)]
#[inline]
pub(crate) fn must_align<T>(ptr: *const T) {
    let actual = (ptr as usize) % align_of::<T>() == 0;
    assert!(actual);
}

#[allow(dead_code)]
#[inline]
pub(crate) fn cals_size_with_align(sz: usize, align_sz: usize) -> usize {
    (sz + align_sz) & !align_sz
}