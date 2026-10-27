//! Private sibling staging and no-replace publication for directory outputs.
//!
//! A writer receives [`Transaction::stage`] and must finish its manifest before
//! calling [`Transaction::publish`].  Until the final rename, no incomplete
//! result is visible at the requested destination.
use crate::Result;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};

pub(crate) struct Transaction {
    stage: PathBuf,
    destination: PathBuf,
}

impl Transaction {
    pub(crate) fn new(destination: &Path) -> Result<Self> {
        // Resolve parent aliases once. Refuse files, directories and dangling links.
        if destination.symlink_metadata().is_ok() {
            return Err("output destination already exists; choose a new directory".into());
        }
        let name = destination
            .file_name()
            .ok_or("output needs a new directory name")?;
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let parent = fs::canonicalize(parent)?;
        let destination = parent.join(name);
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..100 {
            let stage = parent.join(format!(
                ".{}.partial-{}-{}",
                name.to_string_lossy(),
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&stage) {
                Ok(()) => return Ok(Self { stage, destination }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err("cannot reserve a unique staging directory".into())
    }

    pub(crate) fn stage(&self) -> &Path {
        &self.stage
    }

    pub(crate) fn publish(self, cancelled: &AtomicBool) -> Result<()> {
        let publication = Instant::now();
        // Some writers own their file handles and some use ordinary filesystem
        // writes. Sweep every regular file so both kinds have a pre-commit
        // writeback check; directory syncs then persist their names.
        fn sync_dirs(path: &Path, cancelled: &AtomicBool) -> Result<()> {
            check_cancelled(cancelled)?;
            for e in fs::read_dir(path)? {
                let e = e?;
                let file_type = e.file_type()?;
                if file_type.is_dir() {
                    sync_dirs(&e.path(), cancelled)?;
                } else if file_type.is_file() {
                    host_sync(&fs::File::open(e.path())?)?;
                } else {
                    return Err("output staging contains an unsupported filesystem entry".into());
                }
            }
            host_sync(&fs::File::open(path)?)?;
            Ok(())
        }
        let t = Instant::now();
        sync_dirs(&self.stage, cancelled)?;
        // Include newly created destination ancestors, not just the immediate
        // parent. Persist the hidden sibling's name before its commit rename.
        for ancestor in self.destination.parent().unwrap().ancestors() {
            check_cancelled(cancelled)?;
            host_sync(&fs::File::open(ancestor)?)?;
        }
        crate::diagnostics::profile_stage("final_directory_sync", t.elapsed().as_secs_f64());
        let barrier = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.stage.join("manifest.json"))?;
        let t = Instant::now();
        device_barrier(&barrier)?;
        crate::diagnostics::profile_stage("prepublish_device_flush", t.elapsed().as_secs_f64());
        check_cancelled(cancelled)?;
        let t = Instant::now();
        rename_new(&self.stage, &self.destination)?;
        crate::diagnostics::profile_stage("atomic_publication", t.elapsed().as_secs_f64());
        // Rename is the commit point. A failure after it leaves the complete
        // output in place and explicitly reports its unconfirmed durability.
        let post_commit = || -> Result<()> {
            let t = Instant::now();
            host_sync(&fs::File::open(self.destination.parent().unwrap())?)?;
            crate::diagnostics::profile_stage("publication_parent_sync", t.elapsed().as_secs_f64());
            let t = Instant::now();
            device_barrier(&barrier)?;
            crate::diagnostics::profile_stage(
                "postpublish_device_flush",
                t.elapsed().as_secs_f64(),
            );
            Ok(())
        };
        post_commit().map_err(|e| format!("complete output was published at {}, but final synchronization failed; durability is unconfirmed: {e}", self.destination.display()))?;
        crate::diagnostics::profile_stage("publication_total", publication.elapsed().as_secs_f64());
        Ok(())
    }
}

fn check_cancelled(flag: &AtomicBool) -> Result<()> {
    if flag.load(Ordering::Acquire) {
        return Err("acquisition cancelled; output was not published".into());
    }
    Ok(())
}

fn host_sync(file: &fs::File) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::fsync(file.as_raw_fd()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(file.sync_all()?)
    }
}

fn device_barrier(file: &fs::File) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(file.sync_all()?)
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.stage.exists() {
            if let Err(e) = fs::remove_dir_all(&self.stage) {
                eprintln!(
                    "tile-ripper: incomplete staging directory retained at {}: {e}",
                    self.stage.display()
                );
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_new(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rename_new(_: &Path, _: &Path) -> Result<()> {
    Err("atomic no-replace output is currently supported on Linux and macOS".into())
}
