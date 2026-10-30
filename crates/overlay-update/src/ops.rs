//! The file operations the install and rollback make, behind a trait so tests can record every
//! one and make any of them fail.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Every operation that changes the disk during an update.
pub trait Ops {
    /// Move `from` to `to`, replacing a file at `to` (same volume: one `MoveFileEx` on Windows).
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()>;
    /// Copy a file and flush the copy to disk. Anything already at `to` is removed first.
    fn copy(&mut self, from: &Path, to: &Path) -> io::Result<()>;
    fn remove_file(&mut self, path: &Path) -> io::Result<()>;
    fn create_dir_all(&mut self, path: &Path) -> io::Result<()>;
    /// Remove an empty folder.
    fn remove_dir(&mut self, path: &Path) -> io::Result<()>;
    /// Replace `path` with `bytes` through a flushed temporary file and a rename.
    fn write_atomic(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()>;
}

/// The real file system. Operations that fail because another program has the file open (an
/// antivirus scan, typically) are retried.
#[derive(Clone, Debug)]
pub struct RealOps {
    pub retries: u32,
    pub delay: Duration,
}

impl Default for RealOps {
    fn default() -> Self {
        RealOps { retries: 10, delay: Duration::from_millis(500) }
    }
}

/// A failure another program may stop causing soon: a sharing or lock violation on Windows
/// (errors 32 and 33), or access denied while a scanner holds the file.
fn transient(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || (cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33)))
}

impl RealOps {
    fn retry<T>(&self, mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
        let mut tries = 0;
        loop {
            match op() {
                Err(e) if transient(&e) && tries < self.retries => {
                    tries += 1;
                    std::thread::sleep(self.delay);
                }
                other => return other,
            }
        }
    }
}

impl Ops for RealOps {
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.retry(|| std::fs::rename(from, to))
    }

    fn copy(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.retry(|| {
            match std::fs::remove_file(to) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            std::fs::copy(from, to)?;
            std::fs::OpenOptions::new().write(true).open(to)?.sync_all()
        })
    }

    fn remove_file(&mut self, path: &Path) -> io::Result<()> {
        self.retry(|| std::fs::remove_file(path))
    }

    fn create_dir_all(&mut self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn remove_dir(&mut self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir(path)
    }

    fn write_atomic(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write as _;
        let name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
        let mut tmp_name = name.to_os_string();
        tmp_name.push(format!(".{}.tmp", std::process::id()));
        let tmp = path.with_file_name(tmp_name);
        let _ = std::fs::remove_file(&tmp);
        {
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        let renamed = self.retry(|| std::fs::rename(&tmp, path));
        if renamed.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        renamed
    }
}

/// What a [`FaultyOps`] was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Rename(PathBuf, PathBuf),
    Copy(PathBuf, PathBuf),
    RemoveFile(PathBuf),
    CreateDirAll(PathBuf),
    RemoveDir(PathBuf),
    WriteAtomic(PathBuf),
}

impl Op {
    /// The paths the operation changes.
    pub fn targets(&self) -> Vec<&Path> {
        match self {
            Op::Rename(a, b) => vec![a, b],
            Op::Copy(_, b) => vec![b],
            Op::RemoveFile(p) | Op::CreateDirAll(p) | Op::RemoveDir(p) | Op::WriteAtomic(p) => vec![p],
        }
    }
}

/// Real operations, recorded, with an optional failure: operation number `fail_at` (counting
/// from 0) and every one after it fail while `fail_rest` is set, otherwise only that one.
/// For tests and fault injection.
#[derive(Debug, Default)]
pub struct FaultyOps {
    pub log: Vec<Op>,
    pub fail_at: Option<usize>,
    pub fail_rest: bool,
}

impl FaultyOps {
    pub fn new() -> Self {
        FaultyOps::default()
    }

    /// Fail operation `n` (and every later one with `rest`), as a crash or power loss would.
    pub fn failing_at(n: usize, rest: bool) -> Self {
        FaultyOps { fail_at: Some(n), fail_rest: rest, ..FaultyOps::default() }
    }

    fn step(&mut self, op: Op) -> io::Result<()> {
        let n = self.log.len();
        self.log.push(op);
        match self.fail_at {
            Some(k) if n == k || (self.fail_rest && n > k) => Err(io::Error::other(format!("injected failure at operation {n}"))),
            _ => Ok(()),
        }
    }
}

impl Ops for FaultyOps {
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.step(Op::Rename(from.into(), to.into()))?;
        std::fs::rename(from, to)
    }
    fn copy(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.step(Op::Copy(from.into(), to.into()))?;
        let _ = std::fs::remove_file(to);
        std::fs::copy(from, to).map(drop)
    }
    fn remove_file(&mut self, path: &Path) -> io::Result<()> {
        self.step(Op::RemoveFile(path.into()))?;
        std::fs::remove_file(path)
    }
    fn create_dir_all(&mut self, path: &Path) -> io::Result<()> {
        self.step(Op::CreateDirAll(path.into()))?;
        std::fs::create_dir_all(path)
    }
    fn remove_dir(&mut self, path: &Path) -> io::Result<()> {
        self.step(Op::RemoveDir(path.into()))?;
        std::fs::remove_dir(path)
    }
    fn write_atomic(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.step(Op::WriteAtomic(path.into()))?;
        RealOps { retries: 0, delay: Duration::ZERO }.write_atomic(path, bytes)
    }
}
