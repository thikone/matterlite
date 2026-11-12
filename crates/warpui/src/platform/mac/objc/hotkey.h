// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui/src/platform/mac/objc/hotkey.h` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

#import <AppKit/AppKit.h>
#import <Carbon/Carbon.h>

@interface WarpHotKey : NSObject {
   @public
    EventHotKeyRef _eventHotKey;
   @public
    NSUInteger _keyCode;
   @public
    NSUInteger _modifierKeys;
}

- (instancetype)initWithEventHotKey:(EventHotKeyRef)eventHotKey
                            keyCode:(NSUInteger)keyCode
                       modifierKeys:(NSUInteger)modifierKeys;

- (BOOL)hotKeyKeyAndModifierEquals:(NSUInteger)keyCode modifierKeys:(NSUInteger)modifierKeys;

@end
