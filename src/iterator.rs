use crate::iterator::PreFetchStatus::Prefetched;
use crate::kv::_BADGER_PREFIX;
use crate::types::{ArcRW, Channel, Closer, TArcMx, TArcRW};
use crate::{hex_str, ValueStruct, KV};
use crate::{
    value_log::{MetaBit, ValuePointer},
    Decode, MergeIterator, Result, Xiterator, EMPTY_SLICE,
};
use crate::y::{parse_ts, parse_key};

use atomic::Atomic;

use std::fmt::{Debug, Display, Formatter};
use std::future::Future;

use std::pin::Pin;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::{io::Cursor, sync::atomic::AtomicU64};
use tokio::io::AsyncWriteExt;
use tokio::sync::{RwLockReadGuard, RwLockWriteGuard};

#[derive(Debug, PartialEq, Copy, Clone)]
pub(crate) enum PreFetchStatus {
    Empty,
    Prefetched,
}

#[derive(Clone, Debug)]
pub struct KVItem {
    inner: TArcRW<KVItemInner>,
}

impl From<KVItemInner> for KVItem {
    fn from(value: KVItemInner) -> Self {
        Self {
            inner: TArcRW::new(tokio::sync::RwLock::new(value)),
        }
    }
}
// impl Deref for KVItem {
//     type Target = tokio::sync::RwLock<KVItemInner>;
//
//     fn deref(&self) -> &Self::Target {
//         self.inner.as_ref()
//     }
// }

impl KVItem {
    pub async fn key(&self) -> Vec<u8> {
        let inner = self.rl().await;
        inner.key().to_vec()
    }

    pub async fn value(&self) -> Result<Vec<u8>> {
        let inner = self.rl().await;
        inner.get_value().await
    }

    pub async fn has_value(&self) -> bool {
        let inner = self.rl().await;
        inner.has_value()
    }

    pub async fn counter(&self) -> u64 {
        let inner = self.rl().await;
        inner.counter()
    }

    pub async fn user_meta(&self) -> u8 {
        let inner = self.rl().await;
        inner.user_meta()
    }

    pub async fn rl(&self) -> RwLockReadGuard<'_, KVItemInner> {
        self.inner.read().await
    }

    pub(crate) async fn wl(&self) -> RwLockWriteGuard<'_, KVItemInner> {
        self.inner.write().await
    }
}

// Returned during iteration. Both the key() and value() output is only valid until
// iterator.next() is called.
#[derive(Clone)]
pub struct KVItemInner {
    status: Arc<Atomic<PreFetchStatus>>,
    kv: KV,
    key: Vec<u8>,
    // TODO, Opz memory
    vptr: Vec<u8>,
    value: TArcMx<Vec<u8>>,
    meta: u8,
    user_meta: u8,
    cas_counter: Arc<AtomicU64>,
    wg: Closer,
    err: Result<()>,
}

impl Display for KVItemInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("kv")
            .field("key", &hex_str(&self.key))
            .field("meta", &self.meta)
            .field("user_meta", &self.user_meta)
            .field("cas", &self.counter())
            .finish()
    }
}

impl Debug for KVItemInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("kv")
            .field("key", &hex_str(&self.key))
            .field("meta", &self.meta)
            .field("user_meta", &self.user_meta)
            .field("cas", &self.counter())
            .finish()
    }
}

impl KVItemInner {
    pub(crate) fn new(key: Vec<u8>, value: ValueStruct, kv: KV) -> KVItemInner {
        Self {
            status: Arc::new(Atomic::new(PreFetchStatus::Empty)),
            kv,
            key,
            value: TArcMx::new(Default::default()),
            vptr: value.value,
            meta: value.meta,
            user_meta: value.user_meta,
            cas_counter: Arc::new(AtomicU64::new(value.cas_counter)),
            wg: Closer::new("kv".to_owned()),
            err: Ok(()),
        }
    }

    // Returns the key. Remember to copy if you need to access it outside the iteration loop.
    pub(crate) fn key(&self) -> &[u8] {
        &self.key
    }

    // Return value
    pub async fn get_value(&self) -> Result<Vec<u8>> {
        let ch = Channel::new(1);
        self.value(|value| {
            let tx = ch.tx();
            let value = value.to_vec();
            Box::pin(async move {
                tx.send(value).await.unwrap();
                Ok(())
            })
        })
        .await?;
        Ok(ch.recv().await.unwrap())
    }

    // Value retrieves the value of the item from the value log. It calls the
    // consumer function with a slice argument representing the value. In case
    // of error, the consumer function is not called.
    //
    // Note that the call to the consumer func happens synchronously.
    pub(crate) async fn value(
        &self,
        mut consumer: impl FnMut(&[u8]) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>,
    ) -> Result<()> {
        // Wait result
        self.wg.wait().await;
        if self.status.load(Ordering::Acquire) == Prefetched {
            if self.err.is_err() {
                return self.err.clone();
            }
            let value = self.value.lock().await;
            return if value.is_empty() {
                consumer(&EMPTY_SLICE).await
            } else {
                consumer(&value).await
            };
        }
        return self.kv.yield_item_value(self.clone(), consumer).await;
    }

    pub(crate) fn has_value(&self) -> bool {
        if self.meta == 0 && self.vptr.is_empty() {
            return false;
        }
        if (self.meta & MetaBit::BIT_DELETE.bits()) > 0 {
            return false;
        }
        true
    }

    // async fetch value from value_log.
    pub(crate) async fn pre_fetch_value(&self) -> Result<()> {
        let kv = self.kv.clone();
        kv.yield_item_value(self.clone(), |value| {
            let status_wl = self.status.clone();
            let value = value.to_vec();
            let value_wl = self.value.clone();
            Box::pin(async move {
                status_wl.store(Prefetched, Ordering::Release);
                if value.is_empty() {
                    return Ok(());
                }
                let mut value_wl = value_wl.lock().await;
                *value_wl = value;
                Ok(())
            })
        })
        .await
    }

    // Returns approximate size of the key-value pair.
    //
    // This can be called while iterating through a store to quickly estimate the
    // size of a range of key-value pairs (without fetching the corresponding)
    // values).
    pub(crate) fn estimated_size(&self) -> u64 {
        if !self.has_value() {
            return 0;
        }
        if self.meta & MetaBit::BIT_VALUE_POINTER.bits() == 0 {
            return (self.key.len() + self.vptr.len()) as u64;
        }
        let mut vpt = ValuePointer::default();
        vpt.dec(&mut Cursor::new(&self.vptr)).unwrap();
        vpt.len as u64 // includes key length
    }

    // Returns the  CAS counter associated with the value.
    pub fn counter(&self) -> u64 {
        self.cas_counter.load(atomic::Ordering::Acquire)
    }

    // Returns the user_meta set by the user. Typically, this byte, optionally set by the user
    // is used to interpret the value.
    pub(crate) fn user_meta(&self) -> u8 {
        self.user_meta
    }

    pub(crate) fn meta(&self) -> u8 {
        self.meta
    }

    pub(crate) fn vptr(&self) -> &[u8] {
        &self.vptr
    }
}

// Used to set options when iterating over Badger key-value stores.
#[derive(Debug, Clone, Copy)]
pub struct IteratorOptions {
    // Indicates whether we should prefetch values during iteration and store them.
    pub(crate) pre_fetch_values: bool,
    // How may KV pairs to prefetch while iterating. Valid only if PrefetchValues is true.
    pub(crate) pre_fetch_size: isize,
    // Direction of iteration. False is forward, true is backward.
    pub(crate) reverse: bool,
    // If true, iterate through all versions of each key. If false, only show the latest version.
    pub(crate) all_versions: bool,
}

impl Default for IteratorOptions {
    fn default() -> Self {
        DEF_ITERATOR_OPTIONS
    }
}

impl IteratorOptions {
    pub fn new(pre_fetch_values: bool, pre_fetch_size: isize, reverse: bool) -> Self {
        IteratorOptions {
            pre_fetch_values,
            pre_fetch_size,
            reverse,
            all_versions: false,
        }
    }

    pub fn new_with_versions(pre_fetch_values: bool, pre_fetch_size: isize, reverse: bool, all_versions: bool) -> Self {
        IteratorOptions {
            pre_fetch_values,
            pre_fetch_size,
            reverse,
            all_versions,
        }
    }
}

pub(crate) const DEF_ITERATOR_OPTIONS: IteratorOptions = IteratorOptions {
    pre_fetch_size: 100,
    pre_fetch_values: true,
    reverse: false,
    all_versions: false,
};

/// Helps iterating over the KV pairs in a lexicographically sorted order.
/// skiplist,     sst      vlog
///  |             |        |
///  |             |        |
///  IteratorExt  reference
pub struct IteratorExt {
    kv: KV,
    itr: MergeIterator,
    opt: IteratorOptions,
    item: ArcRW<Option<KVItem>>,
    // Cache the prefetch keys, not inlcude current value
    data: ArcRW<std::collections::LinkedList<KVItem>>,
    has_rewind: ArcRW<bool>,
}

// impl futures_core::Stream for IteratorExt {
//     type Item = KVItem;
//
//     fn poll_next(
//         mut self: Pin<&mut Self>,
//         cx: &mut std::task::Context<'_>,
//     ) -> std::task::Poll<Option<Self::Item>> {
//         let mut has_rewind = self.has_rewind.write();
//         if !*has_rewind {
//             *has_rewind = true;
//             match Pin::new(&mut pin!(self.rewind())).poll(cx) {
//                 std::task::Poll::Pending => {
//                     warn!("<<<<Pending>>>>>");
//                     std::task::Poll::Pending
//                 }
//                 std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
//                 std::task::Poll::Ready(t) => std::task::Poll::Ready(t),
//             }
//         } else {
//             match Pin::new(&mut pin!(self.next())).poll(cx) {
//                 std::task::Poll::Pending => {
//                     warn!("<<<<Pending>>>>>");
//                     std::task::Poll::Pending
//                 }
//                 std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
//                 std::task::Poll::Ready(t) => std::task::Poll::Ready(t),
//             }
//         }
//     }
// }

impl IteratorExt {
    pub(crate) fn new(kv: KV, itr: MergeIterator, opt: IteratorOptions) -> IteratorExt {
        IteratorExt {
            kv,
            opt,
            itr,
            data: ArcRW::default(),
            item: Arc::new(Default::default()),
            has_rewind: ArcRW::default(),
        }
    }

    // pub(crate) async fn new_async_iterator(
    //     kv: KV,
    //     itr: MergeIterator,
    //     opt: IteratorOptions,
    // ) -> Box<dyn futures_core::Stream<Item=KVItem>> {
    //     let itr = Self::new(kv, itr, opt);
    //     Box::new(itr)
    // }

    // Seek to the provided key if present. If absent, if would seek to the next smallest key
    // greater than provided if iterating in the forward direction. Behavior would be reversed is
    // iterating backwards.
    pub async fn seek(&self, key: &[u8]) -> Option<KVItem> {
        while let Some(el) = self.data.write().pop_front() {
            el.rl().await.wg.wait().await;
        }
        while let Some(el) = self.itr.seek(key) {
            if el.key().starts_with(_BADGER_PREFIX) {
                continue;
            }
            break;
        }
        self.pre_fetch().await;
        self.item.read().clone()
    }

    // Rewind the iterator cursor all the wy to zero-th position, which would be the
    // smallest key if iterating forward, and largest if iterating backward. It dows not
    // keep track of whether the cursor started with a `seek`.
    pub async fn rewind(&self) -> Option<KVItem> {
        while let Some(el) = self.data.write().pop_front() {
            // Just cleaner to wait before pushing. No ref counting need.
            el.rl().await.wg.wait().await;
        }
        // rewind the iterator
        // rewind, next, rewind?, thie item is who!
        let mut item = self.itr.rewind();
        // filter internal data
        while item.is_some() && item.as_ref().unwrap().key().starts_with(_BADGER_PREFIX) {
            item = self.itr.next();
        }
        // Before every rewind, the item will be reset to None
        self.item.write().take();
        // prefetch item.
        self.pre_fetch().await;
        // return the first el.
        self.item.read().clone()
    }

    // Advance the iterator by one (*NOTICE*: must be rewind when you call self.next())
    pub async fn next(&self) -> Option<KVItem> {
        // Ensure current item has load
        if let Some(el) = self.item.write().take() {
            el.rl().await.wg.wait().await; // Just cleaner to wait before pushing to avoid doing ref counting.
        }
        // Set next item to current
        if let Some(el) = self.data.write().pop_front() {
            self.item.write().replace(el);
        }
        // Advance internal iterator until entry is not deleted
        while let Some(el) = self.itr.next() {
            if el.key().starts_with(_BADGER_PREFIX) {
                continue;
            }
            if el.value().meta & MetaBit::BIT_DELETE.bits() == 0 {
                // Not deleted
                break;
            }
        }
        let item = self.itr.peek();
        if item.is_none() {
            return None;
        }

        let xitem = self.new_item();
        self.fill(xitem.clone()).await;
        self.data.write().push_back(xitem.clone());
        Some(xitem)
    }

    pub async fn peek(&self) -> Option<KVItem> {
        self.item.read().clone()
    }
}

impl IteratorExt {
    // Returns false when iteration is done
    // or when the current key is not prefixed by the specified prefix.
    async fn valid_for_prefix(&self, prefix: &[u8]) -> bool {
        self.item.read().is_some()
            && self
                .item
                .read()
                .as_ref()
                .unwrap()
                .rl()
                .await
                .key()
                .starts_with(prefix)
    }

    // Close the iterator, It is important to call this when you're done with iteration.
    pub async fn close(&self) -> Result<()> {
        // TODO: We could handle this error.
        self.kv.vlog.as_ref().unwrap().decr_iterator_count().await?;
        Ok(())
    }

    // fill the value
    async fn fill(&self, item: KVItem) {
        let vs = self.itr.peek().unwrap();
        let vs = vs.value();
        {
            let mut item = item.wl().await;
            item.meta = vs.meta;
            item.user_meta = vs.user_meta;
            item.cas_counter.store(vs.cas_counter, Ordering::Release);
            item.key.extend(self.itr.peek().as_ref().unwrap().key());
            item.vptr.extend(&vs.value);
            item.value.lock().await.clear();
        }

        // need fetch value, use new coroutine to load value.
        if self.opt.pre_fetch_values {
            item.rl().await.wg.add_running(1);
            tokio::spawn(async move {
                // FIXME we are not handling errors here.
                {
                    let item = item.rl().await;
                    if let Err(err) = item.pre_fetch_value().await {
                        log::error!("Failed to fetch value, {}", err);
                    }
                }
                item.rl().await.wg.done();
            });
        }
    }

    // Prefetch load items.
    async fn pre_fetch(&self) {
        let mut pre_fetch_size = 2;
        if self.opt.pre_fetch_values && self.opt.pre_fetch_size > 1 {
            pre_fetch_size = self.opt.pre_fetch_size;
        }

        let itr = &self.itr;
        let mut count = 0;
        while let Some(item) = itr.peek() {
            if item.key().starts_with(crate::kv::_BADGER_PREFIX) {
                itr.next();
                continue;
            }
            if item.value().meta & MetaBit::BIT_DELETE.bits() > 0 {
                itr.next();
                continue;
            }
            count += 1;
            let xitem = self.new_item();
            // fill a el from itr.peek
            self.fill(xitem.clone()).await;
            if self.item.read().is_none() {
                self.item.write().replace(xitem); // store it
            } else {
                // push prefetch el into cache queue, Notice it not including current item
                self.data.write().push_back(xitem);
            }
            if count == pre_fetch_size {
                break;
            }
            itr.next();
        }
    }

    fn new_item(&self) -> KVItem {
        let inner_item = KVItemInner {
            status: Arc::new(Atomic::new(PreFetchStatus::Empty)),
            kv: self.kv.clone(),
            key: vec![],
            value: TArcMx::new(Default::default()),
            vptr: vec![],
            meta: 0,
            user_meta: 0,
            cas_counter: Arc::new(Default::default()),
            wg: Closer::new("IteratorExt".to_owned()),
            err: Ok(()),
        };
        return KVItem::from(inner_item);
    }

    // Returns false when iteration is done.
    fn valid(&self) -> bool {
        self.item.read().is_some()
    }
}

// ============================================================================
// ALL VERSIONS ITERATOR
// ============================================================================

/// AllVersionsIterator provides iteration over all versions of keys in the database.
/// This is essential for MVCC functionality, allowing users to see the complete
/// version history of each key.
pub struct AllVersionsIterator {
    kv: KV,
    base_iterator: IteratorExt,
    current_key: Option<Vec<u8>>,
    current_versions: Vec<KVItem>,
    version_index: usize,
    opt: IteratorOptions,
}

impl AllVersionsIterator {
    /// Create a new AllVersionsIterator
    pub async fn new(kv: KV, opt: IteratorOptions) -> Result<Self> {
        let base_iterator = kv.new_iterator(opt).await;
        Ok(Self {
            kv,
            base_iterator,
            current_key: None,
            current_versions: Vec::new(),
            version_index: 0,
            opt,
        })
    }

    /// Seek to a specific key and collect all its versions
    pub async fn seek(&mut self, key: &[u8]) -> Result<()> {
        // Reset state
        self.current_key = None;
        self.current_versions.clear();
        self.version_index = 0;

        // Seek to the key
        self.base_iterator.seek(key).await;

        // Collect all versions of this key
        self.collect_versions_for_key(key).await?;

        Ok(())
    }

    /// Rewind to the beginning and start iterating
    pub async fn rewind(&mut self) -> Result<()> {
        self.current_key = None;
        self.current_versions.clear();
        self.version_index = 0;
        self.base_iterator.rewind().await;
        Ok(())
    }

    /// Move to the next key and collect all its versions
    pub async fn next(&mut self) -> Result<()> {
        if self.version_index + 1 < self.current_versions.len() {
            // Move to next version of current key
            self.version_index += 1;
            return Ok(());
        }

        // Move to next key
        self.base_iterator.next().await;
        self.collect_next_key_versions().await?;
        Ok(())
    }

    /// Get the current item (key-value pair with version info)
    pub async fn item(&self) -> Option<&KVItem> {
        self.current_versions.get(self.version_index)
    }

    /// Get the current key (without version timestamp)
    pub async fn key(&self) -> Option<Vec<u8>> {
        if let Some(item) = self.item().await {
            let versioned_key = item.key().await;
            return Some(parse_key(&versioned_key).to_vec());
        }
        None
    }

    /// Get the current value
    pub async fn value(&self) -> Result<Option<Vec<u8>>> {
        if let Some(item) = self.item().await {
            return Ok(Some(item.value().await?));
        }
        Ok(None)
    }

    /// Get the timestamp of the current version
    pub async fn version(&self) -> Option<u64> {
        if let Some(item) = self.item().await {
            let versioned_key = item.key().await;
            return Some(parse_ts(&versioned_key));
        }
        None
    }

    /// Check if the iterator is valid (has more items)
    pub async fn valid(&self) -> bool {
        !self.current_versions.is_empty() && self.version_index < self.current_versions.len()
    }

    /// Close the iterator and release resources
    pub async fn close(&mut self) -> Result<()> {
        self.base_iterator.close().await?;
        Ok(())
    }

    /// Collect all versions for a specific key
    async fn collect_versions_for_key(&mut self, target_key: &[u8]) -> Result<()> {
        self.current_versions.clear();
        self.version_index = 0;

        // Check if we're at the right key
        if let Some(item) = self.base_iterator.peek().await {
            let versioned_key = item.key().await;
            let base_key = parse_key(&versioned_key);
            
            if base_key == target_key {
                // Collect all versions of this key
                self.collect_versions_from_current_position().await?;
                self.current_key = Some(target_key.to_vec());
            }
        }

        Ok(())
    }

    /// Collect versions starting from the current iterator position
    async fn collect_versions_from_current_position(&mut self) -> Result<()> {
        let mut first_key: Option<Vec<u8>> = None;

        while let Some(item) = self.base_iterator.peek().await {
            let versioned_key = item.key().await;
            let base_key = parse_key(&versioned_key);

            // If this is the first key we're processing, set it
            if first_key.is_none() {
                first_key = Some(base_key.to_vec());
            }

            // If we've moved to a different base key, stop collecting
            if first_key.as_ref() != Some(&base_key.to_vec()) {
                break;
            }

            // Add this version to our collection
            self.current_versions.push(item.clone());
            
            // Move to next item
            self.base_iterator.next().await;
        }

        // Sort versions by timestamp (newest first)
        // We need to collect the timestamps first since we can't use await in the closure
        let mut versioned_items: Vec<(u64, KVItem)> = Vec::new();
        for item in self.current_versions.drain(..) {
            let versioned_key = item.key().await;
            let ts = parse_ts(&versioned_key);
            versioned_items.push((ts, item));
        }
        
        // Sort by timestamp (newest first)
        versioned_items.sort_by(|a, b| b.0.cmp(&a.0));
        
        // Extract the sorted items
        self.current_versions = versioned_items.into_iter().map(|(_, item)| item).collect();

        Ok(())
    }

    /// Move to the next key and collect its versions
    async fn collect_next_key_versions(&mut self) -> Result<()> {
        self.current_versions.clear();
        self.version_index = 0;

        if let Some(item) = self.base_iterator.peek().await {
            let versioned_key = item.key().await;
            let base_key = parse_key(&versioned_key);
            self.current_key = Some(base_key.to_vec());
            self.collect_versions_from_current_position().await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod all_versions_tests {
    use super::*;
    use crate::Options;

    #[tokio::test]
    async fn test_all_versions_iterator() {
        let opts = Options::default();
        let kv = KV::open(opts).await.unwrap();

        // Set multiple versions of the same key
        let key = b"test_key";
        kv.set(key.to_vec(), b"version1".to_vec(), 0).await.unwrap();
        kv.set(key.to_vec(), b"version2".to_vec(), 0).await.unwrap();
        kv.set(key.to_vec(), b"version3".to_vec(), 0).await.unwrap();

        // Create AllVersionsIterator
        let opt = IteratorOptions::new_with_versions(true, 100, false, true);
        let mut iter = AllVersionsIterator::new(kv.clone(), opt).await.unwrap();

        // Seek to the key
        iter.seek(key).await.unwrap();

        // Should have 3 versions
        assert!(iter.valid().await);
        
        // Check first version (newest)
        let first_key = iter.key().await.unwrap();
        assert_eq!(first_key, key);
        let first_value = iter.value().await.unwrap().unwrap();
        assert_eq!(first_value, b"version3");

        // Move to next version
        iter.next().await.unwrap();
        assert!(iter.valid().await);
        let second_value = iter.value().await.unwrap().unwrap();
        assert_eq!(second_value, b"version2");

        // Move to next version
        iter.next().await.unwrap();
        assert!(iter.valid().await);
        let third_value = iter.value().await.unwrap().unwrap();
        assert_eq!(third_value, b"version1");

        // Move to next (should be invalid)
        iter.next().await.unwrap();
        assert!(!iter.valid().await);

        iter.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_all_versions_iterator_multiple_keys() {
        let opts = Options::default();
        let kv = KV::open(opts).await.unwrap();

        // Set multiple keys with multiple versions
        kv.set(b"key1".to_vec(), b"key1_v1".to_vec(), 0).await.unwrap();
        kv.set(b"key1".to_vec(), b"key1_v2".to_vec(), 0).await.unwrap();
        kv.set(b"key2".to_vec(), b"key2_v1".to_vec(), 0).await.unwrap();
        kv.set(b"key2".to_vec(), b"key2_v2".to_vec(), 0).await.unwrap();

        // Create AllVersionsIterator
        let opt = IteratorOptions::new_with_versions(true, 100, false, true);
        let mut iter = AllVersionsIterator::new(kv.clone(), opt).await.unwrap();

        // Rewind to beginning
        iter.rewind().await.unwrap();

        // First key should be key1
        assert!(iter.valid().await);
        let first_key = iter.key().await.unwrap();
        assert_eq!(first_key, b"key1");
        let first_value = iter.value().await.unwrap().unwrap();
        assert_eq!(first_value, b"key1_v2"); // Newest version

        // Move to next version of key1
        iter.next().await.unwrap();
        assert!(iter.valid().await);
        let second_value = iter.value().await.unwrap().unwrap();
        assert_eq!(second_value, b"key1_v1");

        // Move to next key (key2)
        iter.next().await.unwrap();
        assert!(iter.valid().await);
        let third_key = iter.key().await.unwrap();
        assert_eq!(third_key, b"key2");
        let third_value = iter.value().await.unwrap().unwrap();
        assert_eq!(third_value, b"key2_v2"); // Newest version

        // Move to next version of key2
        iter.next().await.unwrap();
        assert!(iter.valid().await);
        let fourth_value = iter.value().await.unwrap().unwrap();
        assert_eq!(fourth_value, b"key2_v1");

        iter.close().await.unwrap();
    }
}
