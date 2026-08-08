//! Tuwaiq AI Preview assistant service.
//!
//! This is a real Ring-3 process and a real provider lifecycle contract. It
//! deliberately performs no inference: TuwaiqOS does not yet have the VFS,
//! IPC, capability broker, or model runtime required to host one safely.

#![no_std]
#![no_main]

use core::arch::global_asm;

use hello_user::{syscall, SYS_EXIT, SYS_GETPID};

global_asm!(
    r#"
.global _start
_start:
    call {main}
1:
    jmp 1b
"#,
    main = sym rust_main,
);

trait ModelProvider {
    fn identity(&self) -> &'static [u8];
    fn start(&mut self) -> Result<(), ProviderError>;
    fn generate(&mut self, request: &[u8]) -> Result<&'static [u8], ProviderError>;
    fn shutdown(&mut self);
}

#[derive(Clone, Copy)]
enum ProviderError {
    Unavailable,
    InvalidState,
}

struct LocalDevelopmentProvider {
    running: bool,
}

impl LocalDevelopmentProvider {
    const fn new() -> Self {
        Self { running: false }
    }
}

impl ModelProvider for LocalDevelopmentProvider {
    fn identity(&self) -> &'static [u8] {
        b"local-development-provider"
    }

    fn start(&mut self) -> Result<(), ProviderError> {
        if self.running {
            return Err(ProviderError::InvalidState);
        }
        self.running = true;
        Ok(())
    }

    fn generate(&mut self, _request: &[u8]) -> Result<&'static [u8], ProviderError> {
        if !self.running {
            return Err(ProviderError::InvalidState);
        }
        // No canned answer: absence of a real model runtime is reported as
        // unavailable rather than presented as inference.
        Err(ProviderError::Unavailable)
    }

    fn shutdown(&mut self) {
        self.running = false;
    }
}

extern "C" fn rust_main() -> ! {
    let pid = unsafe { syscall(SYS_GETPID, 0, 0, 0) };
    hello_user::write(b"ai-preview: assistant service started in Ring 3 pid=");
    let mut digits = [0u8; 20];
    hello_user::write(hello_user::u64_to_decimal(pid.max(0) as u64, &mut digits));
    hello_user::write(b"\n");

    let mut provider = LocalDevelopmentProvider::new();
    hello_user::write(b"ai-preview: provider=");
    hello_user::write(provider.identity());
    hello_user::write(b" telemetry=off network=off capabilities=none\n");
    if provider.start().is_err() {
        exit(2);
    }
    match provider.generate(b"status") {
        Err(ProviderError::Unavailable) => hello_user::write(
            b"ai-preview: inference unavailable; no local model runtime is installed\n",
        ),
        _ => exit(3),
    }
    provider.shutdown();
    hello_user::write(b"ai-preview: provider stopped; assistant service exiting cleanly\n");
    exit(0)
}

fn exit(code: i32) -> ! {
    unsafe {
        syscall(SYS_EXIT, code as u64, 0, 0);
    }
    loop {}
}
