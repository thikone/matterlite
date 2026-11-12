// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui/src/platform/mac/objc/hotkey.m` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

#import <AppKit/AppKit.h>
#import <Carbon/Carbon.h>

#import "hotkey.h"

@implementation WarpHotKey

- (instancetype)initWithEventHotKey:(EventHotKeyRef)eventHotKey
                            keyCode:(NSUInteger)keyCode
                       modifierKeys:(NSUInteger)modifierKeys {
    self = [super init];
    if (self) {
        _eventHotKey = eventHotKey;
        _keyCode = keyCode;
        _modifierKeys = modifierKeys;
    }
    return self;
}

- (BOOL)hotKeyKeyAndModifierEquals:(NSUInteger)keyCode modifierKeys:(NSUInteger)modifierKeys {
    return keyCode == _keyCode && modifierKeys == _modifierKeys;
}

@end
