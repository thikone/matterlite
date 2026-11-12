// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warp_util/src/windows.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! Windows-specific utilities.

/// Attaches the current process to the console of the parent process.
///
/// This is useful for command-line interfaces that need to ensure all standard
/// output gets printed correctly when run from a terminal.
pub fn attach_to_parent_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}
