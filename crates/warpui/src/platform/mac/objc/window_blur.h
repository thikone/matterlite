// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui/src/platform/mac/objc/window_blur.h` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

#import <Carbon/Carbon.h>

typedef int CGSWindowID;
typedef void* CGSConnectionID;

extern CGSConnectionID CGSDefaultConnectionForThread(void);

// Typedef for the CGSSetWindowBackgroundBlurRadius function, which is a private
// API.
typedef CGError CGSSetWindowBackgroundBlurRadiusFunction(CGSConnectionID cid, CGSWindowID wid,
                                                         NSUInteger blur);

// Returns a function pointer to the private CGSSetWindowBackgroundBlurRadius
// API, which can be used to set the background blur radius for an NSWindow.
CGSSetWindowBackgroundBlurRadiusFunction* GetCGSSetWindowBackgroundBlurRadiusFunction(void);
