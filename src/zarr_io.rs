//! Timing only: delegate storage and codecs to the existing libraries.
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};
use zarrs::storage::{self as s, ReadableStorageTraits, WritableStorageTraits};
pub(crate) struct Timed<T> {
    pub inner: Arc<T>,
    read: AtomicU64,
    write: AtomicU64,
}
impl<T> Timed<T> {
    pub fn new(inner: Arc<T>) -> Self {
        Self {
            inner,
            read: AtomicU64::new(0),
            write: AtomicU64::new(0),
        }
    }
    pub fn read_seconds(&self) -> f64 {
        self.read.load(Ordering::Relaxed) as f64 * 1e-9
    }
    pub fn write_seconds(&self) -> f64 {
        self.write.load(Ordering::Relaxed) as f64 * 1e-9
    }
    pub fn reset(&self) {
        self.read.store(0, Ordering::Relaxed);
        self.write.store(0, Ordering::Relaxed);
    }
}
impl<T: ReadableStorageTraits> ReadableStorageTraits for Timed<T> {
    fn get(&self, key: &s::StoreKey) -> std::result::Result<s::MaybeBytes, s::StorageError> {
        let t = Instant::now();
        let result = self.inner.get(key);
        self.read
            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        result
    }
    fn get_partial_many<'a>(
        &'a self,
        key: &s::StoreKey,
        ranges: s::byte_range::ByteRangeIterator<'a>,
    ) -> std::result::Result<s::MaybeBytesIterator<'a>, s::StorageError> {
        let t = Instant::now();
        // Consume lazy reads inside the timer; retain library errors unchanged.
        let result = self.inner.get_partial_many(key, ranges).map(|v| {
            v.map(|v| {
                Box::new(v.collect::<Vec<_>>().into_iter())
                    as Box<dyn Iterator<Item = std::result::Result<s::Bytes, s::StorageError>> + 'a>
            })
        });
        self.read
            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        result
    }
    fn size_key(&self, key: &s::StoreKey) -> std::result::Result<Option<u64>, s::StorageError> {
        self.inner.size_key(key)
    }
    fn supports_get_partial(&self) -> bool {
        self.inner.supports_get_partial()
    }
}
impl<T: WritableStorageTraits> WritableStorageTraits for Timed<T> {
    fn set(&self, key: &s::StoreKey, value: s::Bytes) -> std::result::Result<(), s::StorageError> {
        let t = Instant::now();
        let result = self.inner.set(key, value);
        self.write
            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        result
    }
    fn set_partial_many(
        &self,
        key: &s::StoreKey,
        values: s::OffsetBytesIterator,
    ) -> std::result::Result<(), s::StorageError> {
        let t = Instant::now();
        let result = self.inner.set_partial_many(key, values);
        self.write
            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        result
    }
    fn erase(&self, key: &s::StoreKey) -> std::result::Result<(), s::StorageError> {
        self.inner.erase(key)
    }
    fn erase_prefix(&self, prefix: &s::StorePrefix) -> std::result::Result<(), s::StorageError> {
        self.inner.erase_prefix(prefix)
    }
    fn supports_set_partial(&self) -> bool {
        self.inner.supports_set_partial()
    }
}
