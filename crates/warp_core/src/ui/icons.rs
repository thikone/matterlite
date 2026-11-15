// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warp_core/src/ui/icons.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use warpui_core::elements::Icon as WarpUiIcon;

use crate::ui::theme::Fill;

/// Default icon dimensions that apply to all icons used within the ui system.
pub const ICON_DIMENSIONS: f32 = 24.;

/// Icon enum to be used within the app in place of the warpui_core::elements::Icon directly. It
/// abstracts things like svg paths out and provides a utility method to convert into the actual Icon.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    File,
    Plus,
    X,
    Folder,
    Search,
    TextBlock,
    FilterFunnel,
    Regex,
    CaseSensitivity,
    XCircle,
    Info,
    ChevronLeft,
    ChevronRight,
    Paperclip,
    Bell,
    CornerDownLeft,
    PaintBrush,
    ExpandUp,
    ExpandDown,
    ExpandUpAndDown,
    History,
    MessageChatSquare,
    // MatterLite: chats.
    ChevronDown,
    Hash,
    Lock,
    Archive,
    Users,
    Pin,
}

impl From<Icon> for &'static str {
    fn from(icon: Icon) -> &'static str {
        match icon {
            Icon::File => "bundled/svg/file.svg",
            Icon::Plus => "bundled/svg/plus.svg",
            Icon::X => "bundled/svg/x-close.svg",
            Icon::Folder => "bundled/svg/folder.svg",
            Icon::Search => "bundled/svg/search.svg",
            Icon::TextBlock => "bundled/svg/block-text.svg",
            Icon::FilterFunnel => "bundled/svg/filter-funnel.svg",
            Icon::Regex => "bundled/svg/regex.svg",
            Icon::CaseSensitivity => "bundled/svg/case-sensitive.svg",
            Icon::XCircle => "bundled/svg/x-circle.svg",
            Icon::Info => "bundled/svg/info.svg",
            Icon::ChevronLeft => "bundled/svg/chevron-left.svg",
            Icon::ChevronRight => "bundled/svg/chevron-right.svg",
            Icon::Paperclip => "bundled/svg/paperclip.svg",
            Icon::Bell => "bundled/svg/bell.svg",
            Icon::CornerDownLeft => "bundled/svg/corner-down-left.svg",
            Icon::PaintBrush => "bundled/svg/brush-01.svg",
            Icon::ExpandUp => "bundled/svg/expand-up.svg",
            Icon::ExpandDown => "bundled/svg/expand-down.svg",
            Icon::ExpandUpAndDown => "bundled/svg/expand-up-and-down.svg",
            Icon::History => "bundled/svg/history.svg",
            Icon::MessageChatSquare => "bundled/svg/message-chat-square.svg",
            Icon::ChevronDown => "bundled/svg/chevron-down.svg",
            Icon::Hash => "bundled/svg/hash-02.svg",
            Icon::Users => "bundled/svg/users-02.svg",
            Icon::Pin => "bundled/svg/pin-01.svg",
            // Drawn for MatterLite (Warp has no lock or archive icon).
            Icon::Lock => "matterlite/svg/lock.svg",
            Icon::Archive => "matterlite/svg/archive.svg",
        }
    }
}

impl Icon {
    pub fn to_warpui_icon(self, color: Fill) -> WarpUiIcon {
        WarpUiIcon::new(self.into(), color.into_solid())
    }

    pub fn icon_for_key(key: &str) -> Option<WarpUiIcon> {
        match key {
            "⏎" => Some(Self::CornerDownLeft.to_warpui_icon(Fill::black())),
            _ => None,
        }
    }
}
