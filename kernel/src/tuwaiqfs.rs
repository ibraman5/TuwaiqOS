//! TuwaiqFS v2 — persistent filesystem on the boot disk.
//!
//! ## Disk layout (512-byte sectors)
//!
//! ```text
//! LBA 0–8191     Bootloader / kernel area (do not modify)
//! LBA 8192       Superblock
//! LBA 8193–8464  Legacy data slots (reserved)
//! LBA 8465–8712  Metadata blob (tree serialization, 248 sectors)
//! ```
//!
//! v2 stores the **entire directory tree** (nested paths included) in the
//! metadata region so files, directories, and metadata survive reboot.
//!
//! ## Metadata length tracking (Phase 1 bugfix)
//!
//! Earlier builds inferred where the metadata blob ended by scanning for a
//! zero byte followed by nothing but zero padding. That heuristic silently
//! corrupted every reboot: a serialized record's own `content_len` field is
//! a little-endian `u16`, so any file under 256 bytes produces a zero byte
//! (the length's high byte) immediately followed by real, non-zero content
//! -- indistinguishable, under the old heuristic, from "data ends here, the
//! rest is padding". The scan then fell back to treating the *entire*
//! 512-byte sector as data, the deserializer choked on the trailing zero
//! bytes it misread as more records, and `fs::init()`'s error fallback
//! silently substituted an empty filesystem. Every reboot looked like data
//! loss because, functionally, it was: this is what actually caused
//! `write hello.txt hello` + `reboot` + `cat hello.txt` to fail before this
//! fix, despite being the project's own documented validation example.
//!
//! The fix stores the real blob length explicitly in the superblock
//! instead of inferring it, so no scan or heuristic is needed at all.

use alloc::string::String;
use alloc::vec::Vec;

use crate::ata;

pub const SUPERBLOCK_LBA: u32 = 8192;
pub const METADATA_LBA: u32 = 8465;
pub const METADATA_SECTORS: u32 = 248;
pub const MAX_METADATA_BYTES: usize = (METADATA_SECTORS * 512) as usize;
/// The v2 record format stores file length as a little-endian `u16`.
/// Keeping the limit at that format boundary preserves compatibility with
/// every existing v2 volume while allowing native ELF files to be stored.
pub const MAX_FILE_SIZE: usize = u16::MAX as usize;
pub const VERSION: u32 = 2;
/// Bounds parser-owned node allocations independently of metadata byte size.
/// Existing volumes produced by TuwaiqOS remain far below this limit.
const MAX_NODE_COUNT: usize = 1024;

const MAGIC: [u8; 8] = *b"TQFSv2\0\0";
const TREE_MAGIC: [u8; 4] = *b"TREE";

/// Superblock field offsets (all little-endian).
const SB_METADATA_LEN_OFFSET: usize = 20;

/// Serialized node loaded from or written to disk.
pub enum FsNode {
    File { content: Vec<u8> },
    Dir { children: Vec<(String, FsNode)> },
}

/// Load TuwaiqFS from disk, formatting if needed.
pub fn mount() -> Result<FsNode, &'static str> {
    let mut superblock = [0u8; 512];
    ata::read_sector(SUPERBLOCK_LBA, &mut superblock)?;

    if superblock[..8] != MAGIC {
        crate::serial_println!("tuwaiqfs: no valid superblock found, formatting region");
        format_region()?;
        return Ok(empty_root());
    }

    let version = u32::from_le_bytes(superblock[8..12].try_into().unwrap());
    let metadata_lba = u32::from_le_bytes(superblock[12..16].try_into().unwrap());
    let metadata_sectors = u32::from_le_bytes(superblock[16..20].try_into().unwrap());
    if version != VERSION || metadata_lba != METADATA_LBA || metadata_sectors != METADATA_SECTORS {
        return Err("invalid TuwaiqFS superblock geometry or version");
    }

    let metadata_len = read_metadata_len(&superblock);
    if metadata_len > MAX_METADATA_BYTES {
        return Err("TuwaiqFS metadata length exceeds reserved region");
    }
    crate::serial_println!(
        "tuwaiqfs: mounting existing tree, {} bytes of metadata",
        metadata_len
    );
    load_tree(metadata_len)
}

fn read_metadata_len(superblock: &[u8; 512]) -> usize {
    let bytes = &superblock[SB_METADATA_LEN_OFFSET..SB_METADATA_LEN_OFFSET + 4];
    u32::from_le_bytes(bytes.try_into().unwrap()) as usize
}

fn format_region() -> Result<(), &'static str> {
    let superblock = build_superblock(0);
    ata::write_sector(SUPERBLOCK_LBA, &superblock)?;

    let empty = [0u8; 512];
    for sector in METADATA_LBA..METADATA_LBA + METADATA_SECTORS {
        ata::write_sector(sector, &empty)?;
    }
    Ok(())
}

fn build_superblock(metadata_len: u32) -> [u8; 512] {
    let mut superblock = [0u8; 512];
    superblock[..8].copy_from_slice(&MAGIC);
    superblock[8..12].copy_from_slice(&VERSION.to_le_bytes());
    superblock[12..16].copy_from_slice(&METADATA_LBA.to_le_bytes());
    superblock[16..20].copy_from_slice(&METADATA_SECTORS.to_le_bytes());
    superblock[SB_METADATA_LEN_OFFSET..SB_METADATA_LEN_OFFSET + 4]
        .copy_from_slice(&metadata_len.to_le_bytes());
    superblock
}

/// Persist the full filesystem tree to disk.
pub fn sync_tree(root: &FsNode) -> Result<(), &'static str> {
    let blob = serialize_tree(root)?;
    if blob.len() > MAX_METADATA_BYTES {
        return Err("filesystem metadata too large");
    }
    write_metadata(&blob)?;

    // The superblock's recorded length is what makes the next mount able to
    // read back exactly `blob.len()` bytes without guessing -- see the
    // module doc comment for why guessing was actively wrong.
    let superblock = build_superblock(blob.len() as u32);
    ata::write_sector(SUPERBLOCK_LBA, &superblock)
}

fn load_tree(metadata_len: usize) -> Result<FsNode, &'static str> {
    if metadata_len == 0 {
        return Ok(empty_root());
    }
    let blob = read_metadata(metadata_len)?;
    if blob.len() < 4 || blob[..4] != TREE_MAGIC {
        return Err("corrupt TuwaiqFS metadata: bad TREE magic");
    }
    deserialize_tree(&blob[4..])
}

fn empty_root() -> FsNode {
    FsNode::Dir {
        children: Vec::new(),
    }
}

fn serialize_tree(root: &FsNode) -> Result<Vec<u8>, &'static str> {
    let mut blob = Vec::new();
    blob.extend_from_slice(&TREE_MAGIC);
    flatten_tree("", root, &mut blob)?;
    Ok(blob)
}

fn flatten_tree(path: &str, node: &FsNode, out: &mut Vec<u8>) -> Result<(), &'static str> {
    match node {
        FsNode::File { content } => {
            write_record(out, 1, path, Some(content))?;
        }
        FsNode::Dir { children } => {
            if !path.is_empty() {
                write_record(out, 2, path, None)?;
            }
            for (name, child) in children {
                let child_path = join_path(path, name);
                flatten_tree(&child_path, child, out)?;
            }
        }
    }
    Ok(())
}

fn join_path(base: &str, name: &str) -> String {
    if base.is_empty() {
        String::from(name)
    } else {
        let mut path = String::from(base);
        path.push('/');
        path.push_str(name);
        path
    }
}

fn write_record(
    out: &mut Vec<u8>,
    kind: u8,
    path: &str,
    content: Option<&[u8]>,
) -> Result<(), &'static str> {
    let path_bytes = path.as_bytes();
    if path_bytes.is_empty() || path_bytes.len() > 120 {
        return Err("invalid path");
    }
    out.push(kind);
    out.push(path_bytes.len() as u8);
    out.extend_from_slice(path_bytes);
    if kind == 1 {
        let bytes = content.ok_or("missing file content")?;
        if bytes.len() > MAX_FILE_SIZE {
            return Err("file too large");
        }
        let len = bytes.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(bytes);
    }
    Ok(())
}

fn deserialize_tree(data: &[u8]) -> Result<FsNode, &'static str> {
    let mut root = empty_root();
    let mut offset = 0;
    let mut node_count = 0usize;

    while offset < data.len() {
        node_count = node_count.checked_add(1).ok_or("node count overflow")?;
        if node_count > MAX_NODE_COUNT {
            return Err("too many metadata records");
        }
        if offset + 2 > data.len() {
            return Err("truncated record header");
        }
        let kind = data[offset];
        let path_len = data[offset + 1] as usize;
        offset += 2;

        if offset + path_len > data.len() {
            return Err("truncated record path");
        }
        let path = core::str::from_utf8(&data[offset..offset + path_len])
            .map_err(|_| "invalid path in metadata")?;
        validate_metadata_path(path)?;
        offset += path_len;

        let node = if kind == 1 {
            if offset + 2 > data.len() {
                return Err("truncated file record");
            }
            let content_len = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
            offset += 2;
            if offset + content_len > data.len() {
                return Err("truncated file content");
            }
            let mut content = Vec::new();
            content
                .try_reserve_exact(content_len)
                .map_err(|_| "file content allocation failed")?;
            content.extend_from_slice(&data[offset..offset + content_len]);
            offset += content_len;
            FsNode::File { content }
        } else if kind == 2 {
            FsNode::Dir {
                children: Vec::new(),
            }
        } else {
            return Err("unknown record kind");
        };

        insert_at_path(&mut root, path, node)?;
    }

    Ok(root)
}

fn insert_at_path(root: &mut FsNode, path: &str, node: FsNode) -> Result<(), &'static str> {
    let mut parts = Vec::new();
    parts
        .try_reserve_exact(path.bytes().filter(|byte| *byte == b'/').count() + 1)
        .map_err(|_| "metadata path allocation failed")?;
    parts.extend(path.split('/').filter(|part| !part.is_empty()));
    if parts.is_empty() {
        return Ok(());
    }

    let mut current = root;
    for (index, part) in parts.iter().enumerate() {
        let is_last = index + 1 == parts.len();
        match current {
            FsNode::Dir { children } => {
                if is_last {
                    if children.iter().any(|(name, _)| name == part) {
                        return Err("duplicate metadata path");
                    }
                    children
                        .try_reserve(1)
                        .map_err(|_| "metadata child allocation failed")?;
                    children.push((try_owned_name(part)?, node));
                    return Ok(());
                }

                // The canonical serializer emits a directory record before
                // every descendant. Requiring that order prevents corrupt
                // paths from synthesizing uncounted implicit nodes.
                let pos = children
                    .iter()
                    .position(|(name, _)| name == part)
                    .ok_or("metadata parent directory missing")?;
                current = &mut children[pos].1;
            }
            FsNode::File { .. } => return Err("path conflict"),
        }
    }
    Ok(())
}

fn try_owned_name(value: &str) -> Result<String, &'static str> {
    let mut owned = String::new();
    owned
        .try_reserve_exact(value.len())
        .map_err(|_| "metadata name allocation failed")?;
    owned.push_str(value);
    Ok(owned)
}

fn validate_metadata_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty()
        || path.len() > 120
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path.bytes().any(|byte| byte == 0 || byte < 0x20)
    {
        return Err("invalid metadata path");
    }
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." || component.len() > 64 {
            return Err("invalid metadata path component");
        }
    }
    Ok(())
}

/// Read exactly `len` bytes of metadata back from disk. `len` comes from
/// the superblock (see `sync_tree`), so no scanning or end-of-data
/// guessing is needed -- every byte read is known to be real data.
fn read_metadata(len: usize) -> Result<Vec<u8>, &'static str> {
    if len > MAX_METADATA_BYTES {
        return Err("metadata length exceeds reserved region");
    }
    let mut blob = Vec::new();
    blob.try_reserve_exact(len)
        .map_err(|_| "metadata buffer allocation failed")?;
    let mut sector_buf = [0u8; 512];

    let mut sector = METADATA_LBA;
    while blob.len() < len {
        ata::read_sector(sector, &mut sector_buf)?;
        let remaining = len - blob.len();
        let take = remaining.min(512);
        blob.extend_from_slice(&sector_buf[..take]);
        sector += 1;
    }

    Ok(blob)
}

fn write_metadata(blob: &[u8]) -> Result<(), &'static str> {
    let mut sector_buf = [0u8; 512];
    let mut offset = 0;

    for sector_index in 0..METADATA_SECTORS {
        sector_buf.fill(0);
        let remaining = blob.len().saturating_sub(offset);
        let copy_len = remaining.min(512);
        if copy_len > 0 {
            sector_buf[..copy_len].copy_from_slice(&blob[offset..offset + copy_len]);
            offset += copy_len;
        }
        ata::write_sector(METADATA_LBA + sector_index, &sector_buf)?;
    }
    Ok(())
}
