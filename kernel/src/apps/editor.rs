//! Simple file viewer / editor helper.

use alloc::string::String;
use alloc::vec::Vec;

use crate::vfs;

/// Show a file and editing instructions.
pub fn handle(args: &str) -> Result<Vec<String>, &'static str> {
    let name = args.trim();
    if name.is_empty() {
        return Err("usage: editor <file>");
    }

    let content = vfs::shell_read(name)?;
    let mut lines = Vec::new();
    lines.push(format_line("Editing: ", name));
    lines.push(String::from("---"));
    if content.is_empty() {
        lines.push(String::from("(empty file)"));
    } else {
        lines.push(content);
    }
    lines.push(String::from("---"));
    lines.push(String::from("Use: write <file> <text>"));
    Ok(lines)
}

fn format_line(prefix: &str, name: &str) -> String {
    let mut line = String::from(prefix);
    line.push_str(name);
    line
}
