This is zarrs_filesystem 0.3.13 from the Cargo registry, retaining its original
MIT/Apache licensing and storage implementation. Cargo.toml is the registry's
normalized manifest. Source changes are limited to one default-off option and
the corresponding synchronization branch in the existing write function.

The [current complete patch and file hashes](../../docs/consolidation/vendor/vendor-audit.json)
bind this fork to the exact published archive. A standalone test lock is retained;
six tests pass under Rust 1.91 during consolidation. Keep the pinned fork until
an equivalent upstream option passes the existing durability and failure gates.

`FilesystemStoreOptions::defer_device_flush(true)` replaces macOS per-write
`File::sync_all()` with POSIX `fsync()` on the same still-open writer handle.
Other platforms and default users retain upstream behavior. There is no
unsynchronized-write option. The scientific-output transaction alone opts in;
it synchronizes directories and requires explicit F_FULLFSYNC barriers before
and after atomic publication. See docs/durable-output/report.md in the parent
project for protocol, measurements, failure tests and limitations.

Do not replace per-write fsync with a later reopen/fsync pass: some asynchronous
write-back errors can be lost when the writing file descriptor has closed.
No Zarr encoding, storage-key mapping, read/write implementation, locking,
handle caching or codec behavior has been replaced.
