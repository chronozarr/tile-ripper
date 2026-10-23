//! Bounded exact source-range persistence. No credentials or delivery cursor.
use crate::{fail, Request, Result};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Mutex, time::Instant};

const IDENTITY_CAP: usize = 64 * 1024;
const HEADER: usize = 88;
const MAGIC: &[u8; 8] = b"TRANGE01";

/// Fail-closed limits. Bytes count logical managed files, including headers;
/// filesystem blocks/inodes and unrelated caller files are outside this count.
/// A range exceeding a limit fails the acquisition; there is no eviction.
#[derive(Clone, Debug)]
pub struct Options {
    pub directory: PathBuf,
    pub max_bytes: u64,
    pub max_entry_bytes: usize,
    pub max_entries: usize,
}
impl Options {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            max_bytes: 64 * 1024 * 1024,
            max_entry_bytes: 8 * 1024 * 1024,
            max_entries: 2048,
        }
    }
    pub(crate) fn reservation(&self) -> Result<usize> {
        if self.max_bytes == 0
            || self.max_entry_bytes == 0
            || self.max_entry_bytes > 16 * 1024 * 1024
            || self.max_entries == 0
            || self.max_entries > 16384
        {
            return fail("invalid checkpoint limits (entry <=16 MiB, entries 1..16384)");
        }
        // Conservative BTreeSet nodes plus bounded JSON/serialization/scratch.
        // Range bytes use the existing admitted transport/decoder buffer.
        Ok(self.max_entries * 512 + IDENTITY_CAP * 16 + 65536)
    }
}

/// Serialize borrowed inputs into a bounded buffer before constructing a Value.
/// A caller's large strings/vectors must not first be cloned into our index.
pub(crate) fn bounded_identity(value: &impl serde::Serialize) -> Result<Value> {
    Ok(serde_json::from_slice(&bounded_bytes(
        value,
        IDENTITY_CAP / 2,
    )?)?)
}
fn bounded_bytes(value: &impl serde::Serialize, cap: usize) -> Result<Vec<u8>> {
    struct Limited {
        bytes: Vec<u8>,
        cap: usize,
    }
    impl std::io::Write for Limited {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self
                .bytes
                .len()
                .checked_add(bytes.len())
                .is_none_or(|n| n > self.cap)
            {
                return Err(std::io::Error::other("checkpoint identity bound"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = Limited {
        bytes: Vec::with_capacity(4096),
        cap,
    };
    serde_json::to_writer(&mut buffer, value)
        .map_err(|_| "checkpoint acquisition identity exceeds metadata bound")?;
    Ok(buffer.bytes)
}
pub(crate) fn scientific_identity(request: &Request, quality: Value) -> Result<Value> {
    #[derive(serde::Serialize)]
    struct Identity<'a> {
        interpretation_version: &'static str,
        source: &'a crate::Source,
        spatial: &'a crate::Spatial,
        time: &'a Option<crate::Time>,
        variables: &'a [crate::Variable],
        quality: &'a [crate::QualityFilter],
        grid: &'a crate::Grid,
        operations: &'a [crate::Transform],
        aligned_quality: &'a Value,
        numeric_semantics: &'static str,
    }
    bounded_identity(&Identity {
        interpretation_version:"native-hdf5-stored-values-v1", source:&request.source,
        spatial:&request.spatial,time:&request.time,variables:&request.variables,
        quality:&request.quality,grid:&request.grid,operations:&request.operations,
        aligned_quality:&quality,
        numeric_semantics:"stored numeric bits; independent fill masks; explicit coordinates; no automatic scale/offset",
    })
}
pub(crate) fn hash(bytes: &[u8]) -> [u8; 32] {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .try_into()
        .unwrap()
}
fn name(start: u64, len: u64) -> String {
    format!("range-{start}-{len}")
}

#[derive(Default, serde::Serialize)]
struct Counts {
    reused_ranges: u64,
    reused_bytes: u64,
    retained_ranges_reused: u64,
    retained_bytes_reused: u64,
    written_ranges: u64,
    written_bytes: u64,
    read_seconds: f64,
    write_seconds: f64,
}
struct Bound {
    digest: [u8; 32],
    size: u64,
    // true = retained from an earlier invocation; false = written this time.
    entries: std::collections::BTreeMap<(u64, u64), bool>,
    disk_bytes: u64,
}
struct State {
    bound: Option<Bound>,
    failed: bool,
    counts: Counts,
}
/// Internal, process-held store. An exclusive lock spans prepare/stream/finish.
pub(crate) struct Store {
    options: Options,
    scientific: Value,
    access: Value,
    directory: private_fs::Directory,
    state: Mutex<State>,
}
impl Store {
    pub(crate) fn open(options: Options, scientific: Value, access: Value) -> Result<Self> {
        options.reservation()?;
        if serde_json::to_vec(&scientific)?.len() + serde_json::to_vec(&access)?.len()
            > IDENTITY_CAP / 2
        {
            return fail("checkpoint acquisition identity exceeds metadata bound");
        }
        let directory = private_fs::Directory::open(&options.directory)?;
        Ok(Self {
            options,
            scientific,
            access,
            directory,
            state: Mutex::new(State {
                bound: None,
                failed: false,
                counts: Counts::default(),
            }),
        })
    }
    pub(crate) fn reservation(&self) -> usize {
        self.options.reservation().expect("validated limits")
    }
    /// Called only after fresh canonical HEAD/version validation, before reads.
    pub(crate) fn bind(&self, canonical: &str, etag: &str, size: u64) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.failed {
            return fail("checkpoint invalidated by an earlier failure");
        }
        #[derive(serde::Serialize)]
        struct Identity<'a> {
            checkpoint_schema: u32,
            canonical: &'a str,
            etag: &'a str,
            object_length: u64,
            access_policy: &'a Value,
            scientific: &'a Value,
        }
        let identity = bounded_bytes(
            &Identity {
                checkpoint_schema: 1,
                canonical,
                etag,
                object_length: size,
                access_policy: &self.access,
                scientific: &self.scientific,
            },
            IDENTITY_CAP,
        )?;
        let digest = hash(&identity);
        if let Some(bound) = &state.bound {
            return if bound.digest == digest {
                Ok(())
            } else {
                fail("checkpoint object binding changed")
            };
        }
        let result = (|| {
            let existing = self.directory.read_identity()?;
            if existing.as_ref().is_some_and(|v| v != &identity) {
                return fail("checkpoint source/request/access-policy mismatch; choose a fresh checkpoint directory");
            }
            let mut bound = Bound {
                digest,
                size,
                entries: Default::default(),
                disk_bytes: identity.len() as u64,
            };
            self.directory
                .scan(self.options.max_entries * 2 + 8, |filename| {
                    if filename == "identity.json" || filename == "lock" {
                        return Ok(());
                    }
                    if filename == ".pending-entry" || filename == ".pending-identity" {
                        // Only known, private, ordinary files in our managed namespace.
                        self.directory.discard_temporary(filename)?;
                        return Ok(());
                    }
                    let Some((start, len)) = parse_name(filename) else {
                        return fail("unexpected checkpoint file; preserved without modification");
                    };
                    if existing.is_none() {
                        return fail("checkpoint ranges without acquisition identity");
                    }
                    if len == 0
                        || len > self.options.max_entry_bytes as u64
                        || start.checked_add(len).is_none_or(|end| end > size)
                        || bound.entries.len() >= self.options.max_entries
                    {
                        return fail("checkpoint entry exceeds offset/length/count limits");
                    }
                    let header = self.directory.header(filename, len)?;
                    check_header(&header, start, len, &digest)?;
                    bound.disk_bytes = bound
                        .disk_bytes
                        .checked_add(HEADER as u64 + len)
                        .ok_or("checkpoint byte overflow")?;
                    if bound.disk_bytes > self.options.max_bytes {
                        return fail("checkpoint disk byte limit exceeded");
                    }
                    bound.entries.insert((start, len), true);
                    Ok(())
                })?;
            if bound.disk_bytes > self.options.max_bytes {
                return fail("checkpoint disk byte limit exceeded");
            }
            if existing.is_none() {
                self.directory.install_identity(&identity)?;
            }
            Ok(bound)
        })();
        match result {
            Ok(bound) => {
                state.bound = Some(bound);
                Ok(())
            }
            Err(error) => {
                state.failed = true;
                Err(error)
            }
        }
    }
    pub(crate) fn read(&self, start: u64, len: usize) -> Result<Option<Vec<u8>>> {
        let clock = Instant::now();
        let mut state = self.state.lock().unwrap();
        if state.failed {
            return fail("checkpoint invalidated by an earlier failure");
        }
        let result = (|| {
            let bound = state
                .bound
                .as_ref()
                .ok_or("checkpoint not source-validated")?;
            if len == 0
                || len > self.options.max_entry_bytes
                || start
                    .checked_add(len as u64)
                    .is_none_or(|end| end > bound.size)
            {
                return fail("checkpoint range exceeds entry/offset bounds");
            }
            if !bound.entries.contains_key(&(start, len as u64)) {
                return Ok(None);
            }
            let (header, body) = self.directory.read_entry(&name(start, len as u64), len)?;
            check_header(&header, start, len as u64, &bound.digest)?;
            if header[56..88] != hash(&body) {
                return fail("checkpoint content digest mismatch");
            }
            Ok(Some(body))
        })();
        state.counts.read_seconds += clock.elapsed().as_secs_f64();
        match &result {
            Ok(Some(body)) => {
                state.counts.reused_ranges += 1;
                state.counts.reused_bytes += body.len() as u64;
                if state
                    .bound
                    .as_ref()
                    .and_then(|b| b.entries.get(&(start, len as u64)))
                    == Some(&true)
                {
                    state.counts.retained_ranges_reused += 1;
                    state.counts.retained_bytes_reused += body.len() as u64;
                }
            }
            Err(_) => state.failed = true,
            _ => {}
        }
        result
    }
    /// The caller provides only complete, already transport-validated bytes.
    pub(crate) fn write(&self, start: u64, body: &[u8]) -> Result<()> {
        let clock = Instant::now();
        let mut state = self.state.lock().unwrap();
        if state.failed {
            return fail("checkpoint invalidated by an earlier failure");
        }
        let result = (|| {
            let bound = state
                .bound
                .as_mut()
                .ok_or("checkpoint not source-validated")?;
            let len = body.len() as u64;
            if body.is_empty()
                || body.len() > self.options.max_entry_bytes
                || start.checked_add(len).is_none_or(|end| end > bound.size)
            {
                return fail("checkpoint range exceeds entry/offset bounds");
            }
            if bound.entries.contains_key(&(start, len)) {
                return Ok(false);
            }
            let disk = bound
                .disk_bytes
                .checked_add(HEADER as u64 + len)
                .ok_or("checkpoint byte overflow")?;
            if bound.entries.len() >= self.options.max_entries || disk > self.options.max_bytes {
                return fail("checkpoint disk byte or entry count limit reached");
            }
            let mut header = [0u8; HEADER];
            header[..8].copy_from_slice(MAGIC);
            header[8..16].copy_from_slice(&start.to_le_bytes());
            header[16..24].copy_from_slice(&len.to_le_bytes());
            header[24..56].copy_from_slice(&bound.digest);
            header[56..88].copy_from_slice(&hash(body));
            self.directory
                .install_entry(&name(start, len), &header, body)?;
            bound.entries.insert((start, len), false);
            bound.disk_bytes = disk;
            Ok(true)
        })();
        state.counts.write_seconds += clock.elapsed().as_secs_f64();
        match result {
            Ok(true) => {
                state.counts.written_ranges += 1;
                state.counts.written_bytes += body.len() as u64;
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(error) => {
                state.failed = true;
                Err(error)
            }
        }
    }
    pub(crate) fn stats(&self) -> Value {
        let state = self.state.lock().unwrap();
        json!({"schema_version":1,"reuse":"exact source ranges; scientific stream restarts; tiles may repeat",
            "limit_policy":"fail acquisition; no eviction or uncached fallback",
            "limits":{"logical_disk_bytes":self.options.max_bytes,"entry_bytes":self.options.max_entry_bytes,
                "entries":self.options.max_entries,"working_reservation_bytes":self.reservation(),"active_writes":1,"open_files_max":5},
            "disk_bytes":state.bound.as_ref().map_or(0, |b| b.disk_bytes),
            "entries":state.bound.as_ref().map_or(0, |b| b.entries.len()),"failed":state.failed,"counts":state.counts})
    }
}
fn parse_name(value: &str) -> Option<(u64, u64)> {
    let (a, b) = value.strip_prefix("range-")?.split_once('-')?;
    let start = a.parse().ok()?;
    let len = b.parse().ok()?;
    (name(start, len) == value).then_some((start, len))
}
fn check_header(header: &[u8; HEADER], start: u64, len: u64, digest: &[u8; 32]) -> Result<()> {
    if &header[..8] != MAGIC
        || header[8..16] != start.to_le_bytes()
        || header[16..24] != len.to_le_bytes()
        || header[24..56] != *digest
    {
        return fail("checkpoint entry offset/length/version identity mismatch");
    }
    Ok(())
}

#[cfg(unix)]
mod private_fs {
    use super::*;
    use std::{
        ffi::{CStr, CString},
        fs::File,
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
        path::{Component, Path},
    };
    pub(super) struct Directory {
        fd: File,
        _lock: File,
    }
    fn cstr(value: &str) -> Result<CString> {
        Ok(CString::new(value).map_err(|_| "invalid checkpoint filename")?)
    }
    fn opened(fd: libc::c_int) -> Result<File> {
        if fd < 0 {
            return fail(
                "checkpoint filesystem operation failed (permissions, space or unsafe path)",
            );
        }
        // SAFETY: successful open/openat returns a new owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    fn private(metadata: &std::fs::Metadata, directory: bool) -> Result<()> {
        // SAFETY: geteuid has no preconditions.
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || if directory {
                !metadata.is_dir()
            } else {
                !metadata.is_file() || metadata.nlink() != 1
            }
        {
            return fail(
                "checkpoint requires private owned directories and single-link ordinary files",
            );
        }
        Ok(())
    }
    fn dir_at(parent: &File, part: &CStr, create: bool) -> Result<File> {
        // SAFETY: valid descriptors and NUL-terminated relative component.
        let mut raw = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                part.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if raw < 0
            && create
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
        {
            // SAFETY: same pinned parent and component; never follows a symlink.
            if unsafe { libc::mkdirat(parent.as_raw_fd(), part.as_ptr(), 0o700) } != 0 {
                return fail("cannot create private checkpoint directory");
            }
            raw = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    part.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
        }
        opened(raw)
    }
    impl Directory {
        pub(super) fn open(path: &Path) -> Result<Self> {
            if path.as_os_str().as_bytes().len() > 4096 {
                return fail("checkpoint path exceeds bounds");
            }
            let path = std::path::absolute(path).map_err(|_| "invalid checkpoint directory")?;
            if path.components().any(|c| matches!(c, Component::ParentDir)) {
                return fail("checkpoint parent traversal is unsupported");
            }
            let mut parent = File::open("/")?;
            let parts = path
                .components()
                .filter_map(|p| {
                    if let Component::Normal(v) = p {
                        Some(v)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if parts.is_empty() {
                return fail("choose a private checkpoint directory");
            }
            for (i, part) in parts.iter().enumerate() {
                let part = CString::new(part.as_bytes()).map_err(|_| "invalid checkpoint path")?;
                parent = dir_at(&parent, &part, i + 1 == parts.len())?;
            }
            private(&parent.metadata()?, true)?;
            let fd = dir_at(&parent, &cstr("tile-ripper-checkpoint-v1")?, true)?;
            private(&fd.metadata()?, true)?;
            let lock_name = cstr("lock")?;
            // SAFETY: pinned directory, constant basename, no-follow regular-file check below.
            let lock = opened(unsafe {
                libc::openat(
                    fd.as_raw_fd(),
                    lock_name.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_NOFOLLOW
                        | libc::O_NONBLOCK
                        | libc::O_CLOEXEC,
                    0o600,
                )
            })?;
            private(&lock.metadata()?, false)?;
            if lock.metadata()?.len() != 0 {
                return fail("invalid checkpoint lock file");
            }
            // SAFETY: flock applies to this live owned lock descriptor; lock is never unlinked.
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return fail("checkpoint is already in use by another acquisition");
            }
            Ok(Self { fd, _lock: lock })
        }
        fn read_file(&self, name: &str) -> Result<Option<File>> {
            let name = cstr(name)?;
            // SAFETY: fixed/generated single basename relative to pinned directory.
            let fd = unsafe {
                libc::openat(
                    self.fd.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            let file = opened(fd)?;
            private(&file.metadata()?, false)?;
            Ok(Some(file))
        }
        pub(super) fn read_identity(&self) -> Result<Option<Vec<u8>>> {
            let Some(mut file) = self.read_file("identity.json")? else {
                return Ok(None);
            };
            if file.metadata()?.len() > IDENTITY_CAP as u64 {
                return fail("checkpoint identity exceeds metadata bound");
            }
            let mut bytes = Vec::new();
            Read::by_ref(&mut file)
                .take(IDENTITY_CAP as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > IDENTITY_CAP {
                return fail("checkpoint identity exceeds metadata bound");
            }
            Ok(Some(bytes))
        }
        pub(super) fn header(&self, name: &str, len: u64) -> Result<[u8; HEADER]> {
            let mut file = self
                .read_file(name)?
                .ok_or("checkpoint entry disappeared")?;
            if file.metadata()?.len() != HEADER as u64 + len {
                return fail("checkpoint entry file length mismatch");
            }
            let mut header = [0; HEADER];
            file.read_exact(&mut header)?;
            Ok(header)
        }
        pub(super) fn read_entry(&self, name: &str, len: usize) -> Result<([u8; HEADER], Vec<u8>)> {
            let mut file = self
                .read_file(name)?
                .ok_or("checkpoint entry disappeared")?;
            if file.metadata()?.len() != (HEADER + len) as u64 {
                return fail("checkpoint entry file length mismatch");
            }
            let mut header = [0; HEADER];
            file.read_exact(&mut header)?;
            let mut body = vec![0; len];
            file.read_exact(&mut body)?;
            let mut extra = [0];
            if file.read(&mut extra)? != 0 {
                return fail("checkpoint entry grew during read");
            }
            Ok((header, body))
        }
        pub(super) fn discard_temporary(&self, name: &str) -> Result<()> {
            if self.read_file(name)?.is_some() {
                let name = cstr(name)?;
                if unsafe { libc::unlinkat(self.fd.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                    return fail("cannot discard incomplete checkpoint entry");
                }
            }
            Ok(())
        }
        fn install(&self, temporary: &str, name: &str, header: &[u8], body: &[u8]) -> Result<()> {
            // Only absent temporary files are created. Failed writes remain
            // incomplete and require reopening, which discards them safely.
            let temp = cstr(temporary)?;
            let name = cstr(name)?;
            let mut file = opened(unsafe {
                libc::openat(
                    self.fd.as_raw_fd(),
                    temp.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            })?;
            file.write_all(header)?;
            #[cfg(test)]
            super::test_event(if header.is_empty() {
                "identity-header"
            } else {
                "entry-header"
            })?;
            file.write_all(body)?;
            #[cfg(test)]
            super::test_event(if header.is_empty() {
                "identity-body"
            } else {
                "entry-body"
            })?;
            if unsafe { libc::fsync(file.as_raw_fd()) } != 0 {
                return fail("checkpoint file synchronization failed");
            }
            #[cfg(test)]
            super::test_event(if header.is_empty() {
                "identity-synced"
            } else {
                "entry-synced"
            })?;
            // Atomic no-replace rename never exposes a double-link crash state
            // and cannot overwrite an unrelated file or a symlink.
            #[cfg(target_os = "macos")]
            let renamed = unsafe {
                libc::renameatx_np(
                    self.fd.as_raw_fd(),
                    temp.as_ptr(),
                    self.fd.as_raw_fd(),
                    name.as_ptr(),
                    libc::RENAME_EXCL,
                )
            };
            #[cfg(target_os = "linux")]
            let renamed = unsafe {
                libc::renameat2(
                    self.fd.as_raw_fd(),
                    temp.as_ptr(),
                    self.fd.as_raw_fd(),
                    name.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            let renamed = -1;
            if renamed != 0 {
                return fail("cannot atomically finalize checkpoint record");
            }
            #[cfg(test)]
            super::test_event(if header.is_empty() {
                "identity-published"
            } else {
                "entry-published"
            })?;
            if unsafe { libc::fsync(self.fd.as_raw_fd()) } != 0 {
                return fail("checkpoint directory synchronization failed");
            }
            Ok(())
        }
        pub(super) fn install_identity(&self, identity: &[u8]) -> Result<()> {
            self.install(".pending-identity", "identity.json", &[], identity)
        }
        pub(super) fn install_entry(&self, name: &str, header: &[u8], body: &[u8]) -> Result<()> {
            self.install(".pending-entry", name, header, body)
        }
        pub(super) fn scan(
            &self,
            limit: usize,
            mut visit: impl FnMut(&str) -> Result<()>,
        ) -> Result<()> {
            // fdopendir owns its duplicate; readdir names are copied before the
            // next call. There is no unbounded directory listing allocation.
            let duplicate = unsafe { libc::dup(self.fd.as_raw_fd()) };
            if duplicate < 0 {
                return fail("cannot scan checkpoint directory");
            }
            let raw = unsafe { libc::fdopendir(duplicate) };
            if raw.is_null() {
                unsafe { libc::close(duplicate) };
                return fail("cannot scan checkpoint directory");
            }
            struct Dir(*mut libc::DIR);
            impl Drop for Dir {
                fn drop(&mut self) {
                    unsafe { libc::closedir(self.0) };
                }
            }
            let dir = Dir(raw);
            let mut count = 0;
            loop {
                #[cfg(target_os = "macos")]
                unsafe {
                    *libc::__error() = 0;
                }
                #[cfg(target_os = "linux")]
                unsafe {
                    *libc::__errno_location() = 0;
                }
                let entry = unsafe { libc::readdir(dir.0) };
                if entry.is_null() {
                    if std::io::Error::last_os_error()
                        .raw_os_error()
                        .is_some_and(|v| v != 0)
                    {
                        return fail("checkpoint directory scan failed");
                    }
                    break;
                }
                let value = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }
                    .to_str()
                    .map_err(|_| "invalid checkpoint file name")?;
                if value == "." || value == ".." {
                    continue;
                }
                count += 1;
                if count > limit {
                    return fail("checkpoint directory entry scan bound exceeded");
                }
                visit(value)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
thread_local! {static DISK_FULL: std::cell::Cell<bool> = const {std::cell::Cell::new(false)};}
#[cfg(test)]
fn test_event(stage: &str) -> Result<()> {
    if stage == "entry-header" && DISK_FULL.with(|v| v.replace(false)) {
        return Err(std::io::Error::from_raw_os_error(libc::ENOSPC).into());
    }
    if std::env::var("TILE_RIPPER_UNIT_CRASH_STAGE")
        .ok()
        .as_deref()
        == Some(stage)
    {
        // Test-only process exit; no Drop cleanup can conceal an incomplete file.
        std::process::exit(91);
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt},
        sync::atomic::{AtomicUsize, Ordering},
    };
    // Subprocess crash tests briefly inherit open descriptors before exec.
    // Keep lock/drop/reopen assertions independent of those fork windows.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static SERIAL: Mutex<()> = Mutex::new(());
        SERIAL.lock().unwrap()
    }
    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let p = fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(format!(
                    "tile-ripper-checkpoint-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
            Self(p)
        }
        fn options(&self) -> Options {
            Options::new(&self.0)
        }
        fn managed(&self) -> PathBuf {
            self.0.join("tile-ripper-checkpoint-v1")
        }
        fn store(&self) -> Store {
            Store::open(
                self.options(),
                json!({"selection":[1,2,3],"policy":"good"}),
                json!({"access_version":1}),
            )
            .unwrap()
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn bind(s: &Store) -> Result<()> {
        s.bind("https://archive.test/public.nc", "\"strong-version\"", 1024)
    }
    #[test]
    fn exact_ranges_reopen_with_local_digest_and_no_delivery_cursor() -> Result<()> {
        let _serial = serial();
        let root = Root::new();
        fs::write(root.0.join("unrelated"), b"preserve")?;
        let first = root.store();
        bind(&first)?;
        first.write(4, b"abcd")?;
        assert_eq!(first.read(4, 4)?, Some(b"abcd".to_vec()));
        assert!(first.read(4, 3)?.is_none());
        drop(first);
        let second = root.store();
        bind(&second)?;
        for _ in 0..2 {
            assert_eq!(second.read(4, 4)?, Some(b"abcd".to_vec()));
        }
        assert_eq!(second.stats()["counts"]["reused_bytes"], 8);
        assert_eq!(fs::read(root.0.join("unrelated"))?, b"preserve");
        Ok(())
    }
    #[test]
    fn identity_rejects_source_generation_request_access_and_schema_changes() -> Result<()> {
        let _serial = serial();
        let root = Root::new();
        let s = root.store();
        bind(&s)?;
        s.write(0, b"abc")?;
        drop(s);
        for (scientific, access, etag, size) in [
            (
                json!({"selection":[1,2,3],"policy":"good"}),
                json!({"access_version":1}),
                "\"new-version\"",
                1024,
            ),
            (
                json!({"selection":[1,2,3],"policy":"good"}),
                json!({"access_version":1}),
                "\"strong-version\"",
                1025,
            ),
            (
                json!({"selection":[1,2,4],"policy":"good"}),
                json!({"access_version":1}),
                "\"strong-version\"",
                1024,
            ),
            (
                json!({"selection":[1,2,3],"policy":"bad"}),
                json!({"access_version":1}),
                "\"strong-version\"",
                1024,
            ),
            (
                json!({"selection":[1,2,3],"policy":"good"}),
                json!({"access_version":2}),
                "\"strong-version\"",
                1024,
            ),
        ] {
            let s = Store::open(root.options(), scientific, access)?;
            assert!(s
                .bind("https://archive.test/public.nc", etag, size)
                .is_err());
            assert!(s.read(0, 3).is_err());
        }
        let path = root.managed().join("identity.json");
        let mut v: Value = serde_json::from_slice(&fs::read(&path)?)?;
        v["checkpoint_schema"] = 2.into();
        fs::write(path, serde_json::to_vec(&v)?)?;
        assert!(bind(&root.store()).is_err());
        Ok(())
    }
    #[test]
    fn corrupt_truncated_offsets_versions_and_local_bytes_fail_closed() -> Result<()> {
        let _serial = serial();
        for byte in [0, 8, 16, 24, 56, 88] {
            let root = Root::new();
            let s = root.store();
            bind(&s)?;
            s.write(0, b"abcd")?;
            drop(s);
            let path = root.managed().join("range-0-4");
            let mut b = fs::read(&path)?;
            b[byte] ^= 1;
            fs::write(&path, b)?;
            let s = root.store();
            if byte < 56 {
                assert!(bind(&s).is_err());
            } else {
                bind(&s)?;
                assert!(s.read(0, 4).is_err());
                assert!(s.read(0, 4).is_err());
            }
        }
        for length in [0, 87, 89, 93] {
            let root = Root::new();
            let s = root.store();
            bind(&s)?;
            s.write(0, b"abcd")?;
            drop(s);
            fs::OpenOptions::new()
                .write(true)
                .open(root.managed().join("range-0-4"))?
                .set_len(length)?;
            assert!(bind(&root.store()).is_err());
        }
        Ok(())
    }
    #[test]
    fn private_no_follow_no_hardlinks_and_unknown_file_preservation() -> Result<()> {
        let _serial = serial();
        use std::os::unix::fs::symlink;
        for filename in ["identity.json", "lock", "range-0-4", ".pending-entry"] {
            let root = Root::new();
            let s = root.store();
            bind(&s)?;
            s.write(0, b"abcd")?;
            drop(s);
            let trap = root.0.join("trap");
            fs::write(&trap, b"do not touch")?;
            let path = root.managed().join(filename);
            let _ = fs::remove_file(&path);
            symlink(&trap, &path)?;
            let result = Store::open(
                root.options(),
                json!({"selection":[1,2,3],"policy":"good"}),
                json!({"access_version":1}),
            );
            if let Ok(s) = result {
                assert!(bind(&s).is_err());
            }
            assert_eq!(fs::read(&trap)?, b"do not touch");
        }
        let root = Root::new();
        let s = root.store();
        bind(&s)?;
        drop(s);
        fs::write(root.managed().join("unrelated"), b"preserve")?;
        assert!(bind(&root.store()).is_err());
        assert_eq!(fs::read(root.managed().join("unrelated"))?, b"preserve");
        let root = Root::new();
        let s = root.store();
        bind(&s)?;
        s.write(0, b"abcd")?;
        drop(s);
        fs::hard_link(root.managed().join("range-0-4"), root.0.join("hardlink"))?;
        assert!(bind(&root.store()).is_err());
        let root = Root::new();
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o755))?;
        assert!(Store::open(root.options(), json!({}), json!({})).is_err());
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o700))?;
        symlink(&root.0, root.0.join("alias"))?;
        assert!(Store::open(Options::new(root.0.join("alias")), json!({}), json!({})).is_err());
        Ok(())
    }
    #[test]
    fn concurrent_store_ownership_releases_after_drop() -> Result<()> {
        let _serial = serial();
        let root = Root::new();
        let s = root.store();
        let child = std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", "checkpoint::tests::crash_child", "--ignored"])
            .env("TILE_RIPPER_UNIT_EXPECT_LOCK", "1")
            .env("TILE_RIPPER_UNIT_CRASH_ROOT", &root.0)
            .output()?;
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(Store::open(root.options(), json!({}), json!({}))
            .err()
            .unwrap()
            .to_string()
            .contains("already in use"));
        drop(s);
        let _next = root.store();
        Ok(())
    }
    #[test]
    fn limits_and_enospc_are_terminal_and_partial_writes_never_replay() -> Result<()> {
        let _serial = serial();
        let oversized = "x".repeat(1024 * 1024);
        assert!(bounded_identity(&oversized).is_err());
        assert_eq!(
            bounded_identity(&json!({"source":"small"}))?,
            json!({"source":"small"})
        );
        for limit in ["bytes", "entries", "entry"] {
            let root = Root::new();
            let mut o = root.options();
            o.max_entries = 1;
            o.max_entry_bytes = 4;
            let s = Store::open(o.clone(), json!({}), json!({}))?;
            bind(&s)?;
            s.write(0, b"abcd")?;
            o.max_bytes = s.stats()["disk_bytes"].as_u64().unwrap();
            drop(s);
            if limit == "entries" {
                o.max_bytes += 1000;
            }
            let s = Store::open(o, json!({}), json!({}))?;
            bind(&s)?;
            assert!(s
                .write(8, if limit == "entry" { b"abcde" } else { b"abcd" })
                .is_err());
            assert!(s.read(0, 4).is_err());
        }
        let root = Root::new();
        let s = root.store();
        bind(&s)?;
        DISK_FULL.with(|v| v.set(true));
        assert!(s
            .write(0, b"abcd")
            .unwrap_err()
            .to_string()
            .contains("space"));
        assert!(!root.managed().join("range-0-4").exists());
        drop(s);
        let s = root.store();
        bind(&s)?;
        assert!(s.read(0, 4)?.is_none());
        s.write(0, b"abcd")?;
        drop(s);
        let s = root.store();
        bind(&s)?;
        fs::set_permissions(root.managed(), fs::Permissions::from_mode(0o500))?;
        assert!(s.write(4, b"abcd").is_err());
        fs::set_permissions(root.managed(), fs::Permissions::from_mode(0o700))?;
        Ok(())
    }
    #[test]
    fn process_exit_at_every_persistence_boundary_preserves_only_complete_records() -> Result<()> {
        let _serial = serial();
        for stage in [
            "identity-header",
            "identity-body",
            "identity-synced",
            "identity-published",
            "entry-header",
            "entry-body",
            "entry-synced",
            "entry-published",
        ] {
            let root = Root::new();
            let status = std::process::Command::new(std::env::current_exe()?)
                .args(["--exact", "checkpoint::tests::crash_child", "--ignored"])
                .env("TILE_RIPPER_UNIT_CRASH_STAGE", stage)
                .env("TILE_RIPPER_UNIT_CRASH_ROOT", &root.0)
                .stdout(std::process::Stdio::null())
                .status()?;
            assert_eq!(status.code(), Some(91), "{stage}");
            let s = root.store();
            bind(&s)?;
            assert_eq!(
                s.read(0, 4)?.is_some(),
                stage == "entry-published",
                "{stage}"
            );
            if stage != "entry-published" {
                s.write(0, b"abcd")?;
            }
            assert_eq!(s.read(0, 4)?, Some(b"abcd".to_vec()));
        }
        Ok(())
    }
    #[test]
    #[ignore = "subprocess helper for abrupt persistence exits"]
    fn crash_child() {
        let path = PathBuf::from(std::env::var_os("TILE_RIPPER_UNIT_CRASH_ROOT").unwrap());
        if std::env::var_os("TILE_RIPPER_UNIT_EXPECT_LOCK").is_some() {
            let result = Store::open(Options::new(path), json!({}), json!({}));
            assert!(result.err().unwrap().to_string().contains("already in use"));
            return;
        }
        let root = Root(path);
        let s = root.store();
        bind(&s).unwrap();
        s.write(0, b"abcd").unwrap();
        panic!("crash boundary was not reached");
    }
}

#[cfg(not(unix))]
mod private_fs {
    use super::*;
    pub(super) struct Directory;
    impl Directory {
        pub(super) fn open(_: &std::path::Path) -> Result<Self> {
            fail("checkpointing requires Unix private filesystem semantics")
        }
        pub(super) fn read_identity(&self) -> Result<Option<Vec<u8>>> {
            unreachable!()
        }
        pub(super) fn scan(&self, _: usize, _: impl FnMut(&str) -> Result<()>) -> Result<()> {
            unreachable!()
        }
        pub(super) fn discard_temporary(&self, _: &str) -> Result<()> {
            unreachable!()
        }
        pub(super) fn header(&self, _: &str, _: u64) -> Result<[u8; HEADER]> {
            unreachable!()
        }
        pub(super) fn read_entry(&self, _: &str, _: usize) -> Result<([u8; HEADER], Vec<u8>)> {
            unreachable!()
        }
        pub(super) fn install_identity(&self, _: &[u8]) -> Result<()> {
            unreachable!()
        }
        pub(super) fn install_entry(&self, _: &str, _: &[u8], _: &[u8]) -> Result<()> {
            unreachable!()
        }
    }
}
