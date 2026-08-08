//! Notes application — simple persistent notes stored on TuwaiqFS.

use alloc::string::String;
use alloc::vec::Vec;

use crate::vfs;

const NOTES_DIR: &str = ".notes";

fn note_path(name: &str) -> String {
    let mut path = String::from(NOTES_DIR);
    path.push('/');
    path.push_str(name);
    path
}

/// Handle `notes` subcommands.
pub fn handle(sub: &str, args: &str) -> Result<Vec<String>, &'static str> {
    let sub = sub.trim();
    match sub {
        "create" => create(args),
        "set" => set(args),
        "delete" => delete(args),
        "list" => list(),
        "show" => show(args),
        "" => Ok(usage_lines()),
        _ => Err("unknown notes command"),
    }
}

fn create(name: &str) -> Result<Vec<String>, &'static str> {
    let name = name.trim();
    if !valid_note_name(name) {
        return Err("usage: notes create <name>");
    }
    if vfs::kind("/", NOTES_DIR).is_err() {
        vfs::create_dir("/", NOTES_DIR)?;
    }
    vfs::write_file("/", &note_path(name), b"")?;
    let mut lines = Vec::new();
    lines.push(format_message("Created note: ", name));
    Ok(lines)
}

fn set(args: &str) -> Result<Vec<String>, &'static str> {
    let args = args.trim();
    let split = args
        .find(char::is_whitespace)
        .ok_or("usage: notes set <name> <text>")?;
    let name = &args[..split];
    let text = args[split..].trim();
    if !valid_note_name(name) || text.is_empty() {
        return Err("usage: notes set <name> <text>");
    }
    if vfs::kind("/", NOTES_DIR).is_err() {
        vfs::create_dir("/", NOTES_DIR)?;
    }
    vfs::write_file("/", &note_path(name), text.as_bytes())?;
    let mut lines = Vec::new();
    lines.push(format_message("Saved note: ", name));
    Ok(lines)
}

fn delete(name: &str) -> Result<Vec<String>, &'static str> {
    let name = name.trim();
    if !valid_note_name(name) {
        return Err("usage: notes delete <name>");
    }
    vfs::remove("/", &note_path(name))?;
    let mut lines = Vec::new();
    lines.push(format_message("Deleted note: ", name));
    Ok(lines)
}

fn list() -> Result<Vec<String>, &'static str> {
    let mut lines = Vec::new();
    match vfs::list_dir("/", NOTES_DIR) {
        Ok(entries) => {
            if entries.is_empty() {
                lines.push(String::from("(no notes)"));
            } else {
                for entry in entries {
                    lines.push(entry);
                }
            }
        }
        Err(_) => lines.push(String::from("(no notes)")),
    }
    Ok(lines)
}

fn show(name: &str) -> Result<Vec<String>, &'static str> {
    let name = name.trim();
    if !valid_note_name(name) {
        return Err("usage: notes show <name>");
    }
    let bytes = vfs::read_file("/", &note_path(name))?;
    let content = String::from(core::str::from_utf8(&bytes).map_err(|_| "note is not UTF-8 text")?);
    let mut lines = Vec::new();
    lines.push(format_message("Note: ", name));
    lines.push(content);
    Ok(lines)
}

fn usage_lines() -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(String::from("Notes commands:"));
    lines.push(String::from("  notes create <name>"));
    lines.push(String::from("  notes set <name> <text>"));
    lines.push(String::from("  notes delete <name>"));
    lines.push(String::from("  notes list"));
    lines.push(String::from("  notes show <name>"));
    lines
}

fn valid_note_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= vfs::NAME_MAX
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.bytes().any(|byte| byte == 0 || byte < 0x20)
}

fn format_message(prefix: &str, name: &str) -> String {
    let mut message = String::from(prefix);
    message.push_str(name);
    message
}
