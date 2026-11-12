// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui/src/platform/mac/objc/fullscreen_queue.h` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

#import <AppKit/AppKit.h>

// Enforces that multiple windows don't transition to fullscreen at
// the same time.
@interface FullscreenWindowManager : NSObject
// Queues a window to be transitioned to fullscreen. Not thread-safe.
- (void)enqueueWindow:(NSWindow *)window;
@end
