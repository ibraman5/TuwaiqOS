//! Phase 6 virtual-filesystem foundation.
//!
//! The VFS owns path semantics and mount dispatch. TuwaiqFS remains the
//! concrete backend at `/`; callers never receive backend nodes or mutate its
//! internal tree directly. The first milestone intentionally exposes only
//! read-only file handles to Ring 3.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::fs;

/// Current global path bound. TuwaiqFS v2 stores a serialized path in one
/// record and caps it at 120 bytes; the VFS must not promise a wider path
/// while that backend is mounted at root.
pub const PATH_MAX: usize = 120;
pub const NAME_MAX: usize = 64;
pub const MAX_EXECUTABLE_SIZE: usize = crate::tuwaiqfs::MAX_FILE_SIZE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    File,
    Directory,
}

/// Backend contract kept deliberately independent of TuwaiqFS internals.
/// Additional mounts can implement this interface without changing syscall,
/// shell, or process path handling.
trait VfsBackend {
    fn kind(&self, absolute_path: &str) -> Result<NodeKind, &'static str>;
    fn list(&self, absolute_path: &str) -> Result<Vec<String>, &'static str>;
    fn read(&self, absolute_path: &str) -> Result<Arc<[u8]>, &'static str>;
    fn create_file(&self, absolute_path: &str) -> Result<(), &'static str>;
    fn create_dir(&self, absolute_path: &str) -> Result<(), &'static str>;
    fn write(&self, absolute_path: &str, bytes: &[u8]) -> Result<(), &'static str>;
    fn sync(&self) -> Result<(), &'static str>;
}

struct TuwaiqRoot;

impl VfsBackend for TuwaiqRoot {
    fn kind(&self, absolute_path: &str) -> Result<NodeKind, &'static str> {
        match fs::kind_at(absolute_path)? {
            fs::EntryKind::File => Ok(NodeKind::File),
            fs::EntryKind::Directory => Ok(NodeKind::Directory),
        }
    }

    fn list(&self, absolute_path: &str) -> Result<Vec<String>, &'static str> {
        fs::list_at(absolute_path)
    }

    fn read(&self, absolute_path: &str) -> Result<Arc<[u8]>, &'static str> {
        fs::read_at(absolute_path)
    }

    fn create_file(&self, absolute_path: &str) -> Result<(), &'static str> {
        fs::create_file_at(absolute_path)
    }

    fn create_dir(&self, absolute_path: &str) -> Result<(), &'static str> {
        fs::create_dir_at(absolute_path)
    }

    fn write(&self, absolute_path: &str, bytes: &[u8]) -> Result<(), &'static str> {
        fs::write_at(absolute_path, bytes)
    }

    fn sync(&self) -> Result<(), &'static str> {
        fs::sync_to_disk()
    }
}

static ROOT_BACKEND: TuwaiqRoot = TuwaiqRoot;
static SHELL_CWD: Mutex<Option<String>> = Mutex::new(None);

pub fn init() {
    fs::init();
    interrupts::without_interrupts(|| {
        *SHELL_CWD.lock() = Some(String::from("/"));
    });
    crate::serial_println!("vfs: mounted {} at /", fs::label());
}

/// Resolve `input` against normalized absolute `cwd`.
///
/// Empty components and `.` are removed. `..` removes one component and is
/// clamped at the VFS root, so no relative path can escape `/`. Backslashes,
/// NUL/control bytes, overlong names, and overlong final paths are rejected.
pub fn normalize(cwd: &str, input: &str) -> Result<String, &'static str> {
    if input.is_empty() {
        return Err("path required");
    }
    if !cwd.starts_with('/') || cwd.len() > PATH_MAX {
        return Err("invalid working directory");
    }
    if input.contains('\\') || input.bytes().any(|byte| byte == 0 || byte < 0x20) {
        return Err("invalid path character");
    }

    let mut normalized = String::new();
    normalized
        .try_reserve_exact(PATH_MAX)
        .map_err(|_| "path allocation failed")?;
    if input.starts_with('/') {
        normalized.push('/');
    } else {
        if cwd.contains('\\')
            || cwd.bytes().any(|byte| byte == 0 || byte < 0x20)
            || cwd.ends_with('/') && cwd != "/"
        {
            return Err("invalid working directory");
        }
        for component in cwd.split('/').filter(|part| !part.is_empty()) {
            validate_component(component)?;
        }
        normalized.push_str(cwd);
    }

    for component in input.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if normalized.len() > 1 {
                    let separator = normalized.rfind('/').unwrap_or(0);
                    normalized.truncate(separator.max(1));
                }
            }
            value => {
                validate_component(value)?;
                let separator_len = usize::from(normalized != "/");
                let new_len = normalized
                    .len()
                    .checked_add(separator_len)
                    .and_then(|length| length.checked_add(value.len()))
                    .ok_or("path length overflow")?;
                if new_len > PATH_MAX {
                    return Err("path too long");
                }
                if separator_len != 0 {
                    normalized.push('/');
                }
                normalized.push_str(value);
            }
        }
    }
    Ok(normalized)
}

fn validate_component(component: &str) -> Result<(), &'static str> {
    if component.is_empty() || component.len() > NAME_MAX {
        return Err("invalid path component length");
    }
    if component == "." || component == ".." {
        return Err("reserved path component");
    }
    Ok(())
}

pub fn kind(cwd: &str, path: &str) -> Result<NodeKind, &'static str> {
    let absolute = normalize(cwd, path)?;
    ROOT_BACKEND.kind(&absolute)
}

pub fn read_file(cwd: &str, path: &str) -> Result<Arc<[u8]>, &'static str> {
    let absolute = normalize(cwd, path)?;
    ROOT_BACKEND.read(&absolute)
}

pub fn list_dir(cwd: &str, path: &str) -> Result<Vec<String>, &'static str> {
    let absolute = normalize(cwd, path)?;
    ROOT_BACKEND.list(&absolute)
}

pub fn create_file(cwd: &str, path: &str) -> Result<(), &'static str> {
    let absolute = normalize(cwd, path)?;
    ROOT_BACKEND.create_file(&absolute)
}

pub fn create_dir(cwd: &str, path: &str) -> Result<(), &'static str> {
    let absolute = normalize(cwd, path)?;
    ROOT_BACKEND.create_dir(&absolute)
}

pub fn write_file(cwd: &str, path: &str, bytes: &[u8]) -> Result<(), &'static str> {
    let absolute = normalize(cwd, path)?;
    ROOT_BACKEND.write(&absolute, bytes)
}

pub fn sync() -> Result<(), &'static str> {
    ROOT_BACKEND.sync()
}

fn shell_cwd() -> Result<String, &'static str> {
    interrupts::without_interrupts(|| {
        SHELL_CWD
            .lock()
            .as_ref()
            .cloned()
            .ok_or("VFS not initialized")
    })
}

pub fn shell_pwd() -> Result<String, &'static str> {
    shell_cwd()
}

pub fn shell_chdir(path: &str) -> Result<String, &'static str> {
    let current = shell_cwd()?;
    let target = normalize(&current, path.trim())?;
    if ROOT_BACKEND.kind(&target)? != NodeKind::Directory {
        return Err("not a directory");
    }
    interrupts::without_interrupts(|| {
        *SHELL_CWD.lock() = Some(target.clone());
    });
    Ok(target)
}

pub fn shell_list(path: Option<&str>) -> Result<Vec<String>, &'static str> {
    let cwd = shell_cwd()?;
    list_dir(&cwd, path.unwrap_or("."))
}

pub fn shell_read(path: &str) -> Result<String, &'static str> {
    let cwd = shell_cwd()?;
    let bytes = read_file(&cwd, path.trim())?;
    let text = core::str::from_utf8(&bytes).map_err(|_| "file is not UTF-8 text")?;
    Ok(text.to_string())
}

pub fn shell_create_file(path: &str) -> Result<(), &'static str> {
    let cwd = shell_cwd()?;
    create_file(&cwd, path.trim())
}

pub fn shell_create_dir(path: &str) -> Result<(), &'static str> {
    let cwd = shell_cwd()?;
    create_dir(&cwd, path.trim())
}

pub fn shell_write(path: &str, text: &str) -> Result<(), &'static str> {
    let cwd = shell_cwd()?;
    write_file(&cwd, path.trim(), text.as_bytes())
}

pub fn completion_candidates(token: &str) -> Result<Vec<String>, &'static str> {
    let cwd = shell_cwd()?;
    let (typed_parent, leaf) = match token.rfind('/') {
        Some(index) => (&token[..=index], &token[index + 1..]),
        None => ("", token),
    };
    let parent_query = if typed_parent.is_empty() {
        "."
    } else {
        typed_parent
    };
    let parent = normalize(&cwd, parent_query)?;
    let entries = ROOT_BACKEND.list(&parent)?;
    let mut matches = Vec::new();
    for entry in entries {
        if entry.trim_end_matches('/').starts_with(leaf) {
            let mut candidate = String::from(typed_parent);
            candidate.push_str(&entry);
            matches.push(candidate);
        }
    }
    Ok(matches)
}

pub fn basename(path: &str) -> &str {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("process")
}

pub fn self_test() -> Result<(), &'static str> {
    let cases = [
        (("/", "/"), "/"),
        (("/home/user", "../bin/./tool"), "/home/bin/tool"),
        (("/home/user", "../../../../etc"), "/etc"),
        (("/", "//apps///hello"), "/apps/hello"),
        (("/a", "."), "/a"),
    ];
    for ((cwd, input), expected) in cases {
        if normalize(cwd, input)? != expected {
            return Err("path normalization mismatch");
        }
    }
    if normalize("/", "").is_ok()
        || normalize("/", "bad\\path").is_ok()
        || normalize("/", "bad\0path").is_ok()
        || normalize("/", &"x".repeat(NAME_MAX + 1)).is_ok()
    {
        return Err("invalid path accepted");
    }
    Ok(())
}
