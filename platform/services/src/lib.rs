//! Tuwaiq Platform service surface placeholders (D0).
//!
//! These modules document the intended API boundary. They intentionally do
//! **not** implement behavior that would pretend unsupported features exist.

#![allow(dead_code)]

pub mod launch {
    //! Application launch mediation (future).
    pub const SURFACE: &str = "tuwaiq.platform.launch";
}

pub mod notify {
    //! Notifications (future).
    pub const SURFACE: &str = "tuwaiq.platform.notify";
}

pub mod fs {
    //! File operations (future).
    pub const SURFACE: &str = "tuwaiq.platform.fs";
}

pub mod permissions {
    //! Permission / policy checks (future).
    pub const SURFACE: &str = "tuwaiq.platform.permissions";
}

pub mod sysinfo {
    //! System information (future).
    pub const SURFACE: &str = "tuwaiq.platform.sysinfo";
}

pub mod ai {
    //! Mediated AI requests (future). Never unrestricted root.
    pub const SURFACE: &str = "tuwaiq.platform.ai";
}

pub mod system {
    //! System actions such as shutdown (future).
    pub const SURFACE: &str = "tuwaiq.platform.system";
}

/// D0: list planned surfaces for documentation/tests.
pub fn planned_surfaces() -> &'static [&'static str] {
    &[
        launch::SURFACE,
        notify::SURFACE,
        fs::SURFACE,
        permissions::SURFACE,
        sysinfo::SURFACE,
        ai::SURFACE,
        system::SURFACE,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaces_are_named() {
        assert!(planned_surfaces().len() >= 7);
    }
}
