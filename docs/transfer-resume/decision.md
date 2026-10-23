# Source transfer resume (schema 1)

Status: implementation contract, 2026-10-22; continues local `60008d3`.

An acquisition can opt into a private directory of complete HTTP source ranges.
It is bound to the canonical unsigned locator, strong ETag, object length,
versioned exact-origin/path access policy, selected variables, index/coordinate
selections and strides, quality policy and the flag definitions used by that
policy, and native scientific interpretation version. ETags remain trusted
provider version validators; local SHA-256 detects checkpoint corruption and
does not independently authenticate provider content.

The four lifecycles are distinct:

1. **Source bytes:** persist only a complete response after the existing status,
   conditional version, range framing, encoding and body-length checks succeed.
   Exact `(offset,length)` matches can be reused. No interval assembly or shared
   dataset cache is introduced.
2. **Scientific stream:** prepare and decode again from the beginning. Every
   previously delivered tile may be delivered again. No cross-process delivery
   cursor is recorded. Metadata page size, worker count, traversal strategy,
   buffer budget and tile edge are execution choices, not scientific identity;
   changed read boundaries may miss the checkpoint.
3. **Consumer state:** the consumer restarts its calculation. It must discard
   provisional results from a failed attempt or independently manage its own
   transaction. Arbitrary state, side effects and partial datasets are outside
   this contract.
4. **Publication:** successful final source validation and `finish()` are required
   before committing a result. A retained byte range never authorizes offline
   success. There is no HDF5 output writer or partial-output recovery.

Each invocation creates an explicitly caller-owned authenticated session and
validates the canonical source online before any checkpoint data can be read.
Since this archive's HEAD signer can be public, each checkpoint invocation also
requires a fresh conditional eight-byte GET through the authenticated session;
that proof always uses the network, even with a complete checkpoint.
Existing preparation, pre-execution and final validation remain mandatory.
Fresh signed routes do not change object identity. Credentials, cookies,
negotiation bodies and signed URLs are never checkpoint fields.

The store uses a private managed subdirectory, a process-held exclusive advisory
lock, bounded identity metadata and an in-memory exact-range index. Each range
is one self-describing file containing its offset, length, acquisition digest,
content digest and bytes. One temporary entry is written, synchronized and
atomically renamed; there is no separate per-entry manifest update that could
detach metadata from bytes. The immutable identity record is installed first.
Unfinished temporary records are ignored and removed only in the managed
namespace. An incompatible identity or corrupt completed record fails closed;
choose another directory for an explicit fresh acquisition.

Limits cover total logical file bytes (including record headers and temporary
writes), per-entry bytes, entry count and conservative index/scratch working
reservation. Reaching a limit or a filesystem error fails the acquisition;
there is no implicit eviction or uncached fallback. Filesystem allocation blocks,
directory/inode overhead and unrelated caller files are outside logical-byte
accounting. All paths are opened relative to pinned directory descriptors with
no-follow checks; owned private Unix directories and ordinary single-link files
are required. Other platforms fail closed. Unrelated caller files are preserved.

The guarantee is local process-crash recovery, with file synchronization and
atomic namespace changes. Tests of process death do not establish durability
against power loss, kernel/filesystem failure, or a malicious process with the
same user's privileges. SHA-256 is an unkeyed corruption check, not protection
against an attacker who can replace both records and their digests.

The no-checkpoint path keeps its existing transport and lifecycle. Authentication
remains explicit and limited to one configured native HDF5 object; ordinary
anonymous preparation never reads credentials.
