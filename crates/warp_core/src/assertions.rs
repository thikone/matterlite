// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warp_core/src/assertions.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

/// Asserts that a condition is true, logging an error if it is not.
///
/// This macro is similar to the standard `debug_assert!` macro, but it logs
/// an error if the condition is not met.  This should generally be preferred
/// over `debug_assert!`, as it will log in production, though should not be
/// used in codepaths where the error log could be produced with high volume.
#[macro_export]
macro_rules! safe_assert {
    ($cond:expr $(,)?) => {{
        debug_assert!($cond);
        match &$cond {
            (cond) => {
                if !(*cond) {
                    $crate::__warp_errors::report_error!($crate::__anyhow::anyhow!("Assertion `{}` failed", stringify!($cond)));
                }
            }
        }
    }};
    ($cond:expr, $($arg:tt)+) => {{
        debug_assert!($cond, $($arg)+);
        match &$cond {
            (cond) => {
                if !(*cond) {
                    $crate::__warp_errors::report_error!($crate::__anyhow::anyhow!("Assertion `{}` failed: {}", stringify!($cond), format_args!($($arg)+)));
                }
            }
        }
    }};
}
pub use safe_assert;

/// Asserts that two expressions are equal, logging an error if they are not.
///
/// This macro is similar to the standard `debug_assert_eq!` macro, but it logs
/// an error if the values are not equal. This should generally be preferred
/// over `debug_assert_eq!`, as it will log in production, though should not be
/// used in codepaths where the error log could be produced with high volume.
#[macro_export]
macro_rules! safe_assert_eq {
    ($left:expr, $right:expr $(,)?) => {{
        debug_assert_eq!($left, $right);
        match (&$left, &$right) {
            (left_val, right_val) => {
                if !(*left_val == *right_val) {
                    $crate::__warp_errors::report_error!($crate::__anyhow::anyhow!("Assertion `{} == {}` failed: expected {left_val}, found {right_val}.", stringify!($left), stringify!($right)));
                }
            }
        }
    }};
    ($left:expr, $right:expr, $($arg:tt)+) => {{
        debug_assert_eq!($left, $right, $($arg)+);
        match (&$left, &$right) {
            (left_val, right_val) => {
                if !(*left_val == *right_val) {
                    $crate::__warp_errors::report_error!($crate::__anyhow::anyhow!("Assertion `{} == {}` failed: expected {left_val}, found {right_val}. {}", stringify!($left), stringify!($right), format_args!($($arg)+)));
                }
            }
        }
    }};
}
pub use safe_assert_eq;
