# Synchronization audit of 0d0cc18

The validated uncompressed Zarr v2 writer retains its application publication
contract. No further synchronization optimization is made in this follow-up.
The existing measurements establish syscall behavior and application failure
handling; they do not establish physical persistence after a power cut.

## Exact implementation change from 82e21b1

The scientific COG sink uses a scoped source copy of **zarrs_filesystem 0.3.13**.
Comparison against the Cargo registry source confirms one new default-false
option, its documentation, and a branch in the existing `set_impl`. The other
Rust source and upstream filesystem tests are identical. Compression, keys,
locking, cached descriptors and ordinary read/write behavior are unchanged.
Other Zarr adapters retain the registry dependency.

Previously every stored object ended with `File::sync_all()` on its writing
descriptor. On macOS with the tested Rust 1.92 this calls `fcntl(F_FULLFSYNC)`;
on Linux it calls `fsync`. The scoped option instead calls POSIX `fsync` on
macOS **on the same descriptor, after writing/truncating and before closing**.
It propagates errors. Linux and default users of the scoped crate retain the
original operation. There is no unsynchronized-write option.

The transaction order in [scientific_output.rs](../../src/scientific_output.rs)
is:

1. Create a unique hidden sibling `.NAME.partial-PID-ID`. Each chunk/metadata
   write synchronizes on its original writer descriptor. The manifest is also
   written and synchronized through this store.
2. Require successful stream finish, final source-version validation, exact
   delivery coverage and cancellation checks before committing the result.
3. Walk staging directories bottom-up and synchronize them. Synchronize the
   destination's ancestors through the root, covering new parent entries and
   the hidden sibling name. Failures propagate before publication.
4. Open the staged manifest read/write and retain this descriptor through the
   rename. Request checked `F_FULLFSYNC` on macOS (`sync_all` on Linux).
5. Poll cancellation immediately before an atomic no-replace sibling rename:
   macOS `renamex_np(RENAME_EXCL)`, Linux `renameat2(RENAME_NOREPLACE)`.
6. Synchronize the destination parent, then request the second checked barrier
   on the still-open manifest descriptor. Return success only after both pass.

The optimization replaces per-write stronger requests with per-write host
synchronization plus two transaction barriers. It also checks directories,
newly created ancestors and publication synchronization. Linux retains its
per-file synchronization and has no claimed speedup.

Opening files later is not substituted for original-descriptor synchronization.
Linux's write-back error reporting depends on descriptor lifetime; a later open
is not equivalent evidence about earlier writes. The directory entry needs its
own synchronization as well. [Linux fsync](https://man7.org/linux/man-pages/man2/fsync.2.html),
[kernel errseq](https://www.kernel.org/doc/html/latest/core-api/errseq.html).

The source comment saying Rust `sync_all` silently falls back to `fsync` was
incorrect for the tested compiler. [Rust 1.92's implementation](https://github.com/rust-lang/rust/blob/1.92.0/library/std/src/sys/fs/unix.rs)
uses `F_FULLFSYNC` directly on Apple targets and propagates failure. This audit
corrects only that comment. The explicit transaction barriers also propagate
failure; their error handling is not a newly stronger alternative to a Rust
fallback. The synchronization calls/order and binaries being assessed are
unchanged.

## Application-level publication

The rename is the commit point. Normal readers of the requested destination
see no destination until the complete staged tree is renamed into place.
The rename atomically refuses any existing destination, including a file,
directory or dangling symlink. No incomplete tree is incrementally published
at that name. This assumes the application's owned staging tree is not modified
by another process; it is not an isolation mechanism against unrelated writers.

Before rename, returned errors/cancellation remove only owned staging.
SIGKILL can leave an identifiable hidden orphan, which retries ignore. A hidden
staging manifest may already say complete; that is not publication. After rename,
failed final synchronization leaves the complete destination and explicitly
reports that publication occurred but durability is unconfirmed. A signal after
the last cancellation poll can race with commit; that path finishes the
post-commit protocol. Success is not inferred merely from a manifest's status.

The retained 31 injected EIO/SIGINT/SIGKILL/race cases and four rerun transaction
unit tests support these application properties. Killing a process does not
simulate losing the OS page cache, filesystem journal or device power.

## Filesystem crash consistency

Same-parent rename gives atomic namespace visibility during normal operation.
Crash persistence additionally depends on synchronized contents, directory
entries and the filesystem/storage stack honoring their documented ordering.
The checked sequence is intended to make a complete tree and its committed name
persistent on a conforming local stack; atomic rename alone cannot establish
that result after an OS crash or power loss.

On macOS, POSIX fsync moves file data/attributes through the host cache; it does
not by itself promise that a drive has emptied its volatile cache. Apple's
`F_FULLFSYNC` requests that additional flush. Using it after all host file/dir
synchronizations relies on the filesystem/device honoring that request and
ordering on the destination storage. [Apple fsync](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html),
[Apple fcntl](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html).
These archived manuals explain the distinction; their historical filesystem
list is not an APFS power-loss certification. The measured host is macOS
26.6.2 arm64 on a local journaled APFS Data volume.

No actual power-cut, kernel-crash or recovery experiment was run. Crash outcomes
of this exact APFS/device/controller combination remain unmeasured. Network
filesystems, virtualized/Docker mounts, controller caches and devices that
ignore or misreport flush completion are not certified by these results. The
Linux container checks establish application compatibility, not storage
persistence or Linux performance.

## What the hardware observations actually are

For each of the three matched warm-local large-window runs, the retained
interposer reports:

| Writer | POSIX fsync requests | F_FULLFSYNC requests | Recorded I/O errors |
| --- | ---: | ---: | ---: |
| 82e21b1 | 0 | 941 | 0 |
| 0d0cc18 | 952 | 2 | 0 |

The C interposer calls the real syscall and records its result and elapsed time.
Both optimized stronger requests returned success; ordinary successful runs
had no injected fault. The counts include output/manifest and relevant
directories. They measure **requests accepted by the OS**, not physical device
flushes. Reported synchronization durations likewise measure time spent in
those calls. A driver may translate, combine or complete them without a
one-to-one physical command or independently observable media write.

There is no drive/controller telemetry, storage-bus command trace, volatile-cache
measurement or physical readback after power removal in these receipts.
Therefore “941 → 2 full flushes” must be read as **941 → 2 observed
F_FULLFSYNC requests**, not as a measured number of hardware flushes or proof
that bytes survived power failure. This qualification does not change the
measured 9.2×/2.3× application completion speedups or justify weaker publication.
