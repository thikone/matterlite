// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui/src/platform/mac/objc/alert.h` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

#import <AppKit/AppKit.h>

NSModalResponse configureAndRunModal(NSAlert* alert, NSApplication* app);
