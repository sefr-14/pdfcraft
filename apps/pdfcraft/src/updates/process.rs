//! Which processes run, and starting them, for the update helper (UPD.1). Windows lists them
//! through `winsafe` (safe Win32 wrappers, already a dependency); Linux reads `/proc` so the
//! helper can be exercised there; elsewhere nothing is listed (updates install only on Windows).

use std::ffi::OsString;
use std::io;
use std::path::Path;

use pdfcraft_overlay_update::helper::{Launched, Launcher, Processes};

/// The two executables an installation runs.
const NAMES: [&str; 2] = ["pdfcraft.exe", "pdfcraft-cli.exe"];

pub struct OsProcesses;

/// Whether `dir` and `other` are the same folder (case-insensitively on Windows).
fn same_dir(dir: &Path, other: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (a, b) = (canon(dir), canon(other));
    if cfg!(windows) { a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() } else { a == b }
}

#[cfg(windows)]
impl Processes for OsProcesses {
    fn alive(&self, pid: u32) -> bool {
        use winsafe::{self as w, co};
        if pid == 0 {
            return false;
        }
        match w::HPROCESS::OpenProcess(co::PROCESS::SYNCHRONIZE, false, pid) {
            Ok(h) => matches!(h.WaitForSingleObject(Some(0)), Ok(co::WAIT::TIMEOUT)),
            Err(_) => false,
        }
    }

    fn using(&self, install: &Path, except: &[u32]) -> Vec<u32> {
        use winsafe::{self as w, co};
        let mut found = Vec::new();
        let Ok(mut snapshot) = w::HPROCESSLIST::CreateToolhelp32Snapshot(co::TH32CS::SNAPPROCESS, None) else {
            return found;
        };
        for entry in snapshot.iter_processes() {
            let Ok(entry) = entry else { break };
            let pid = entry.th32ProcessID;
            if except.contains(&pid) || !NAMES.iter().any(|n| entry.szExeFile().eq_ignore_ascii_case(n)) {
                continue;
            }
            let Ok(h) = w::HPROCESS::OpenProcess(co::PROCESS::QUERY_LIMITED_INFORMATION, false, pid) else { continue };
            let Ok(image) = h.QueryFullProcessImageName(co::PROCESS_NAME::WIN32) else { continue };
            if Path::new(&image).parent().is_some_and(|p| same_dir(p, install)) {
                found.push(pid);
            }
        }
        found
    }
}

#[cfg(all(unix, not(windows)))]
impl Processes for OsProcesses {
    fn alive(&self, pid: u32) -> bool {
        pid != 0 && Path::new(&format!("/proc/{pid}")).exists()
    }

    fn using(&self, install: &Path, except: &[u32]) -> Vec<u32> {
        let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
        dir.filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()))
            .filter(|pid| !except.contains(pid))
            .filter(|pid| {
                std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| {
                    exe.file_name().and_then(|n| n.to_str()).is_some_and(|n| NAMES.iter().any(|m| m.trim_end_matches(".exe") == n || *m == n))
                        && exe.parent().is_some_and(|p| same_dir(p, install))
                })
            })
            .collect()
    }
}

#[cfg(not(any(windows, unix)))]
impl Processes for OsProcesses {
    fn alive(&self, _: u32) -> bool {
        false
    }
    fn using(&self, _: &Path, _: &[u32]) -> Vec<u32> {
        Vec::new()
    }
}

/// Bytes free for this user on the volume holding `dir`, where the system says.
#[cfg(windows)]
pub fn free_space(dir: &Path) -> Option<u64> {
    let mut free = 0u64;
    winsafe::GetDiskFreeSpaceEx(Some(&dir.to_string_lossy()), Some(&mut free), None, None).ok().map(|()| free)
}

#[cfg(not(windows))]
pub fn free_space(_: &Path) -> Option<u64> {
    None
}

/// Starts processes with `std::process`.
pub struct OsLauncher;

struct Child(std::process::Child);

impl Launched for Child {
    fn pid(&self) -> u32 {
        self.0.id()
    }
    fn exited(&mut self) -> Option<bool> {
        match self.0.try_wait() {
            Ok(Some(status)) => Some(status.success()),
            Ok(None) => None,
            // Can't tell: treat as still running (never roll back over a running program).
            Err(_) => None,
        }
    }
}

impl Launcher for OsLauncher {
    fn launch(&mut self, exe: &Path, args: &[OsString]) -> io::Result<Box<dyn Launched>> {
        let dir = exe.parent().unwrap_or(Path::new("."));
        std::process::Command::new(exe).args(args).current_dir(dir).spawn().map(|c| Box::new(Child(c)) as Box<dyn Launched>)
    }
}
