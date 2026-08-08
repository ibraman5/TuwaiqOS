//! Host-side build script (runs on Windows during `cargo build -p tuwaiqos`).
//!
//! This script ONLY wraps an already-built kernel ELF into a BIOS disk image.
//! The kernel is built separately first (see `scripts/build.ps1`) so we do not
//! spawn nested `cargo` while the `bootloader` crate's own build.rs is running.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const KERNEL_TARGET: &str = "x86_64-unknown-none";
const KERNEL_BIN: &str = "kernel";
const IMAGE_NAME: &str = "boot-bios-tuwaiqos.img";

fn main() {
    if let Err(err) = run() {
        eprintln!();
        eprintln!("=== TuwaiqOS image build FAILED ===");
        eprintln!("{err}");
        eprintln!("===================================");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    track_kernel_sources();
    print_build_environment();

    let profile = env_var("PROFILE")?;
    let manifest_dir = PathBuf::from(env_var("CARGO_MANIFEST_DIR")?);
    let target_dir = env_var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("target"));

    let kernel_elf = locate_kernel_elf(&manifest_dir, &target_dir, &profile)
        .ok_or_else(|| kernel_not_found_message(&manifest_dir, &target_dir, &profile))?;
    println!("cargo:rerun-if-changed={}", kernel_elf.display());

    let image_dir = target_dir.join(&profile);
    let image = image_dir.join(IMAGE_NAME);

    print_section("Paths");
    print_kv("manifest_dir", &manifest_dir);
    print_kv("target_dir", &target_dir);
    print_str("profile", &profile);
    print_kv("kernel_elf", &kernel_elf);
    print_kv("output_image", &image);

    print_section("Pre-flight checks");
    verify_kernel_elf(&kernel_elf)?;
    ensure_output_dir(&image_dir)?;
    verify_llvm_objcopy()?;

    print_section("Creating BIOS disk image");
    println!("Calling bootloader::BiosBoot::create_disk_image(...)");

    match bootloader::BiosBoot::new(&kernel_elf).create_disk_image(&image) {
        Ok(()) => {
            println!("create_disk_image returned Ok");
        }
        Err(err) => {
            return Err(format!(
                "bootloader::BiosBoot::create_disk_image failed:\n{err:#}"
            ));
        }
    }

    if !image.exists() {
        return Err(format!(
            "create_disk_image reported success but file is missing:\n  {}",
            image.display()
        ));
    }

    let image_size = fs::metadata(&image)
        .map_err(|e| format!("cannot read image metadata: {e}"))?
        .len();

    print_section("Success");
    println!("Disk image : {}", image.display());
    println!("Image size : {image_size} bytes");

    println!("cargo:rustc-env=TUWAIQOS_BOOT_IMAGE={}", image.display());
    println!("cargo:warning=TuwaiqOS disk image: {}", image.display());

    Ok(())
}

/// Track every kernel source file, not a hand-maintained subset.
///
/// A hardcoded file list here previously omitted several modules (this is
/// how, during Phase 1 interrupt work, `gdt.rs`/`interrupts.rs`/`serial.rs`
/// went unlisted): Cargo saw nothing *this build script itself watches*
/// had changed and skipped rerunning it, silently repackaging a stale
/// kernel ELF into the disk image for several rebuild-and-test cycles.
/// Walking the actual directory tree makes that class of bug structurally
/// impossible -- a new module is tracked the moment its file exists.
fn track_kernel_sources() {
    println!("cargo:rerun-if-changed=kernel/Cargo.toml");
    let mut dirs = std::collections::VecDeque::new();
    dirs.push_back(PathBuf::from("kernel/src"));
    while let Some(dir) = dirs.pop_front() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push_back(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}

fn print_build_environment() {
    print_section("Build environment");

    print_kv_optional("RUSTUP_TOOLCHAIN", option_env("RUSTUP_TOOLCHAIN"));
    print_kv_optional("RUSTUP_HOME", option_env("RUSTUP_HOME"));
    print_kv_optional("CARGO", option_env("CARGO"));
    print_kv_optional("RUSTC", option_env("RUSTC"));
    print_kv_optional("PROFILE", option_env("PROFILE"));
    print_kv_optional("CARGO_MANIFEST_DIR", option_env("CARGO_MANIFEST_DIR"));
    print_kv_optional("CARGO_TARGET_DIR", option_env("CARGO_TARGET_DIR"));

    if let Ok(output) = Command::new("rustup")
        .args(["show", "active-toolchain"])
        .output()
    {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !text.is_empty() {
            println!("active_toolchain : {text}");
        }
    }

    if let Ok(output) = Command::new("rustup").args(["which", "rustc"]).output() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !text.is_empty() {
            println!("rustup which rustc : {text}");
        }
    }
}

fn verify_kernel_elf(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|e| {
        format!(
            "kernel ELF does not exist or is unreadable:\n  {}\n  {e}",
            path.display()
        )
    })?;

    if !metadata.is_file() {
        return Err(format!("kernel path is not a file:\n  {}", path.display()));
    }

    if metadata.len() == 0 {
        return Err(format!("kernel ELF is empty:\n  {}", path.display()));
    }

    println!("kernel ELF exists ({} bytes)", metadata.len());
    Ok(())
}

fn ensure_output_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| {
        format!(
            "failed to create output directory:\n  {}\n  {e}",
            path.display()
        )
    })?;
    println!("output directory ready: {}", path.display());
    Ok(())
}

fn verify_llvm_objcopy() -> Result<(), String> {
    match llvm_tools::LlvmTools::new() {
        Ok(tools) => match tools.tool(&llvm_tools::exe("llvm-objcopy")) {
            Some(path) => {
                println!("llvm-objcopy : {}", path.display());
                Ok(())
            }
            None => Err("llvm-objcopy not found in llvm-tools-preview.\n  \
                 Run: rustup component add llvm-tools-preview --toolchain nightly-2026-06-01"
                .to_string()),
        },
        Err(err) => Err(format!(
            "failed to initialize llvm-tools:\n  {err:?}\n  \
             Run: rustup component add llvm-tools-preview --toolchain nightly-2026-06-01"
        )),
    }
}

fn locate_kernel_elf(manifest_dir: &Path, target_dir: &Path, profile: &str) -> Option<PathBuf> {
    let candidates = [
        target_dir
            .join(KERNEL_TARGET)
            .join(profile)
            .join(KERNEL_BIN),
        target_dir
            .join(KERNEL_TARGET)
            .join(profile)
            .join(format!("{KERNEL_BIN}.exe")),
        manifest_dir
            .join("target")
            .join(KERNEL_TARGET)
            .join(profile)
            .join(KERNEL_BIN),
        manifest_dir
            .join("target")
            .join(KERNEL_TARGET)
            .join(profile)
            .join(format!("{KERNEL_BIN}.exe")),
    ];

    print_section("Kernel ELF search");
    for candidate in &candidates {
        let status = if candidate.exists() {
            "FOUND"
        } else {
            "missing"
        };
        println!("  [{status}] {}", candidate.display());
    }

    candidates.into_iter().find(|path| path.exists())
}

fn kernel_not_found_message(manifest_dir: &Path, target_dir: &Path, profile: &str) -> String {
    format!(
        "kernel ELF not found.\n\n\
         Build the kernel FIRST, then build the disk image:\n\n\
           cargo build --package kernel --target {KERNEL_TARGET}\n\
           cargo build --package tuwaiqos\n\n\
         Or use the helper script:\n\n\
           .\\scripts\\build.ps1\n\n\
         Expected locations (profile={profile}):\n\
           {}\n\
           {}\n\
         manifest_dir: {}\n\
         target_dir  : {}",
        target_dir
            .join(KERNEL_TARGET)
            .join(profile)
            .join(KERNEL_BIN)
            .display(),
        manifest_dir
            .join("target")
            .join(KERNEL_TARGET)
            .join(profile)
            .join(KERNEL_BIN)
            .display(),
        manifest_dir.display(),
        target_dir.display(),
    )
}

fn env_var(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("missing environment variable: {name}"))
}

fn option_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn print_section(title: &str) {
    println!();
    println!("--- {title} ---");
}

fn print_kv(label: &str, path: &Path) {
    println!("{label:16}: {}", path.display());
}

fn print_str(label: &str, value: &str) {
    println!("{label:16}: {value}");
}

fn print_kv_optional(label: &str, value: Option<String>) {
    match value {
        Some(v) => println!("{label:16}: {v}"),
        None => println!("{label:16}: (not set)"),
    }
}
