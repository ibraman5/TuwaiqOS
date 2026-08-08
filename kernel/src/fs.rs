//! TuwaiqFS-backed storage tree.
//!
//! This module is the concrete root-filesystem backend used by `vfs.rs`.
//! It deliberately accepts normalized absolute paths only; path policy,
//! working directories, and mount selection belong to the VFS layer.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use core::sync::atomic::{AtomicBool, Ordering};

use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::tuwaiqfs::{self, FsNode};

/// A named entry inside a directory.
#[derive(Clone)]
enum Entry {
    File { content: Arc<[u8]> },
    Dir { children: Vec<(String, Entry)> },
}

#[derive(Clone)]
struct FileSystem {
    root: Entry,
}

/// The in-memory tree is reachable from the shell and from preempted Ring-3
/// syscalls. Every access therefore uses the same interrupt-safe lock.
/// Expensive ATA I/O is never performed while this lock is held.
static FS: Mutex<Option<Arc<FileSystem>>> = Mutex::new(None);

/// Serializes filesystem mutation without spinning on a single CPU. A writer
/// may be preempted while serializing or waiting for ATA, so another writer
/// must fail with "filesystem busy" rather than spin and prevent the owner
/// from being scheduled again.
static WRITER_ACTIVE: AtomicBool = AtomicBool::new(false);

struct WriterGuard;

impl WriterGuard {
    fn acquire() -> Result<Self, &'static str> {
        WRITER_ACTIVE
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| "filesystem busy")
    }
}

impl Drop for WriterGuard {
    fn drop(&mut self) {
        WRITER_ACTIVE.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    File,
    Directory,
}

pub fn init() {
    let mounted = tuwaiqfs::mount().unwrap_or_else(|reason| {
        crate::serial_println!(
            "fs: mount failed ({}), falling back to an empty filesystem",
            reason
        );
        FsNode::Dir {
            children: Vec::new(),
        }
    });

    let root = match mounted {
        FsNode::Dir { children } => {
            let entries = children
                .into_iter()
                .map(|(name, node)| (name, from_fs_node(node)))
                .collect();
            Entry::Dir { children: entries }
        }
        FsNode::File { .. } => Entry::Dir {
            children: Vec::new(),
        },
    };

    let root = Arc::new(FileSystem { root });
    interrupts::without_interrupts(|| {
        *FS.lock() = Some(root);
    });
}

fn from_fs_node(node: FsNode) -> Entry {
    match node {
        FsNode::File { content } => Entry::File {
            content: Arc::from(content.into_boxed_slice()),
        },
        FsNode::Dir { children } => Entry::Dir {
            children: children
                .into_iter()
                .map(|(name, child)| (name, from_fs_node(child)))
                .collect(),
        },
    }
}

fn to_fs_node(entry: &Entry) -> FsNode {
    match entry {
        Entry::File { content } => FsNode::File {
            content: content.as_ref().to_vec(),
        },
        Entry::Dir { children } => FsNode::Dir {
            children: children
                .iter()
                .map(|(name, child)| (name.clone(), to_fs_node(child)))
                .collect(),
        },
    }
}

fn with_fs<F, R>(f: F) -> Result<R, &'static str>
where
    F: FnOnce(&FileSystem) -> Result<R, &'static str>,
{
    let snapshot = interrupts::without_interrupts(|| {
        let guard = FS.lock();
        guard
            .as_ref()
            .map(Arc::clone)
            .ok_or("filesystem not initialized")
    })?;
    f(&snapshot)
}

fn mutate_and_persist<F>(f: F) -> Result<(), &'static str>
where
    F: FnOnce(&mut FileSystem) -> Result<(), &'static str>,
{
    let _writer = WriterGuard::acquire()?;
    // Clone only the root Arc while locked, then copy tree structure/Arc
    // references, mutate, serialize, and perform ATA I/O with interrupts
    // enabled. Publish only after persistence succeeds: an ATA error leaves
    // the prior in-memory tree untouched. The non-spinning writer guard keeps
    // concurrent Ring-3 mutations from losing updates.
    let snapshot = interrupts::without_interrupts(|| {
        let guard = FS.lock();
        guard
            .as_ref()
            .map(Arc::clone)
            .ok_or("filesystem not initialized")
    })?;
    let mut candidate = (*snapshot).clone();
    f(&mut candidate)?;
    tuwaiqfs::sync_tree(&to_fs_node(&candidate.root))?;
    let published = Arc::new(candidate);
    interrupts::without_interrupts(|| {
        let mut guard = FS.lock();
        if guard.is_none() {
            return Err("filesystem not initialized");
        }
        *guard = Some(published);
        Ok(())
    })
}

impl FileSystem {
    fn entry_at(&self, path: &[String]) -> Result<&Entry, &'static str> {
        let mut node = &self.root;
        for part in path {
            node = find_child(node, part)?;
        }
        Ok(node)
    }

    fn children_at(&self, path: &[String]) -> Result<&Vec<(String, Entry)>, &'static str> {
        match self.entry_at(path)? {
            Entry::Dir { children } => Ok(children),
            Entry::File { .. } => Err("not a directory"),
        }
    }

    fn children_at_mut(
        &mut self,
        path: &[String],
    ) -> Result<&mut Vec<(String, Entry)>, &'static str> {
        let mut node = &mut self.root;
        for part in path {
            node = find_child_mut(node, part)?;
        }
        match node {
            Entry::Dir { children } => Ok(children),
            Entry::File { .. } => Err("not a directory"),
        }
    }

    fn parent_and_name(path: &str) -> Result<(Vec<String>, String), &'static str> {
        let parts = split_absolute_path(path)?;
        let name = parts.last().cloned().ok_or("root has no entry name")?;
        Ok((parts[..parts.len() - 1].to_vec(), name))
    }

    fn kind_at(&self, path: &str) -> Result<EntryKind, &'static str> {
        let parts = split_absolute_path(path)?;
        match self.entry_at(&parts)? {
            Entry::File { .. } => Ok(EntryKind::File),
            Entry::Dir { .. } => Ok(EntryKind::Directory),
        }
    }

    fn list_at(&self, path: &str) -> Result<Vec<String>, &'static str> {
        let parts = split_absolute_path(path)?;
        let children = self.children_at(&parts)?;
        Ok(children
            .iter()
            .map(|(name, entry)| {
                if matches!(entry, Entry::Dir { .. }) {
                    let mut display = name.clone();
                    display.push('/');
                    display
                } else {
                    name.clone()
                }
            })
            .collect())
    }

    fn read_at(&self, path: &str) -> Result<Arc<[u8]>, &'static str> {
        let parts = split_absolute_path(path)?;
        if parts.is_empty() {
            return Err("is a directory");
        }
        match self.entry_at(&parts)? {
            Entry::File { content } => Ok(Arc::clone(content)),
            Entry::Dir { .. } => Err("is a directory"),
        }
    }

    fn create_file_at(&mut self, path: &str) -> Result<(), &'static str> {
        let (parent, name) = Self::parent_and_name(path)?;
        let children = self.children_at_mut(&parent)?;
        if children.iter().any(|(existing, _)| existing == &name) {
            return Err("file or directory already exists");
        }
        children.push((
            name,
            Entry::File {
                content: Arc::from([]),
            },
        ));
        Ok(())
    }

    fn create_dir_at(&mut self, path: &str) -> Result<(), &'static str> {
        let (parent, name) = Self::parent_and_name(path)?;
        let children = self.children_at_mut(&parent)?;
        if children.iter().any(|(existing, _)| existing == &name) {
            return Err("file or directory already exists");
        }
        children.push((
            name,
            Entry::Dir {
                children: Vec::new(),
            },
        ));
        Ok(())
    }

    fn write_at(&mut self, path: &str, bytes: &[u8]) -> Result<(), &'static str> {
        if bytes.len() > tuwaiqfs::MAX_FILE_SIZE {
            return Err("file too large");
        }
        let (parent, name) = Self::parent_and_name(path)?;
        let children = self.children_at_mut(&parent)?;
        match children.iter_mut().find(|(existing, _)| existing == &name) {
            Some((_, Entry::File { content })) => {
                *content = Arc::from(bytes);
            }
            Some((_, Entry::Dir { .. })) => return Err("is a directory"),
            None => children.push((
                name,
                Entry::File {
                    content: Arc::from(bytes),
                },
            )),
        }
        Ok(())
    }

    fn remove_at(&mut self, path: &str) -> Result<(), &'static str> {
        let (parent, name) = Self::parent_and_name(path)?;
        let children = self.children_at_mut(&parent)?;
        let index = children
            .iter()
            .position(|(existing, _)| existing == &name)
            .ok_or("entry not found")?;
        if matches!(&children[index].1, Entry::Dir { children } if !children.is_empty()) {
            return Err("directory not empty");
        }
        children.remove(index);
        Ok(())
    }

    fn metadata_at(&self, path: &str) -> Result<EntryMetadata, &'static str> {
        let parts = split_absolute_path(path)?;
        match self.entry_at(&parts)? {
            Entry::File { content } => Ok(EntryMetadata {
                kind: EntryKind::File,
                size: content.len(),
            }),
            Entry::Dir { children } => Ok(EntryMetadata {
                kind: EntryKind::Directory,
                size: children.len(),
            }),
        }
    }
}

fn split_absolute_path(path: &str) -> Result<Vec<String>, &'static str> {
    if !path.starts_with('/') {
        return Err("backend path must be absolute");
    }
    Ok(path
        .split('/')
        .filter(|component| !component.is_empty())
        .map(String::from)
        .collect())
}

fn find_child<'a>(node: &'a Entry, name: &str) -> Result<&'a Entry, &'static str> {
    match node {
        Entry::Dir { children } => children
            .iter()
            .find(|(entry_name, _)| entry_name == name)
            .map(|(_, entry)| entry)
            .ok_or("entry not found"),
        Entry::File { .. } => Err("not a directory"),
    }
}

fn find_child_mut<'a>(node: &'a mut Entry, name: &str) -> Result<&'a mut Entry, &'static str> {
    match node {
        Entry::Dir { children } => children
            .iter_mut()
            .find(|(entry_name, _)| entry_name == name)
            .map(|(_, entry)| entry)
            .ok_or("entry not found"),
        Entry::File { .. } => Err("not a directory"),
    }
}

pub fn kind_at(path: &str) -> Result<EntryKind, &'static str> {
    with_fs(|fs| fs.kind_at(path))
}

pub fn list_at(path: &str) -> Result<Vec<String>, &'static str> {
    with_fs(|fs| fs.list_at(path))
}

pub fn read_at(path: &str) -> Result<Arc<[u8]>, &'static str> {
    with_fs(|fs| fs.read_at(path))
}

pub fn create_file_at(path: &str) -> Result<(), &'static str> {
    mutate_and_persist(|fs| fs.create_file_at(path))
}

pub fn create_dir_at(path: &str) -> Result<(), &'static str> {
    mutate_and_persist(|fs| fs.create_dir_at(path))
}

pub fn write_at(path: &str, bytes: &[u8]) -> Result<(), &'static str> {
    mutate_and_persist(|fs| fs.write_at(path, bytes))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntryMetadata {
    pub kind: EntryKind,
    /// File length in bytes, or immediate child count for a directory.
    pub size: usize,
}

pub fn metadata_at(path: &str) -> Result<EntryMetadata, &'static str> {
    with_fs(|fs| fs.metadata_at(path))
}

pub fn remove_at(path: &str) -> Result<(), &'static str> {
    mutate_and_persist(|fs| fs.remove_at(path))
}

pub fn sync_to_disk() -> Result<(), &'static str> {
    let _writer = WriterGuard::acquire()?;
    let root = with_fs(|fs| Ok(fs.root.clone()))?;
    tuwaiqfs::sync_tree(&to_fs_node(&root))
}

pub fn label() -> &'static str {
    "TuwaiqFS v2"
}
