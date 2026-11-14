// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `app/src/editor/` (view, model, soft_wrap),
// `app/src/settings/editor.rs`, `app/src/settings/select.rs`, `app/src/util/clipboard.rs`,
// `app/src/util/extensions.rs` and `app/src/terminal/grid_size_util.rs` @ 5bb7bcc6a — adapted
// for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! Warp's text editor view: [`EditorView::single_line`] for search bars and form fields, and
//! [`EditorView::new`] for multi-line input. Each editor owns a CRDT text buffer with undo/redo,
//! multiple selections, soft wrap and IME support.
//!
//! Vim mode, voice input, AI context, image attachments, autosuggestions, command x-ray and
//! shared-session peers are not part of this extraction.

use std::borrow::Cow;
use std::cmp::{self, Ordering};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use enum_iterator::{Sequence, all};
use model::{
    Anchor, Bias, DisplayMap, DrawableSelection, EditorModel, EditorModelEvent, Edits,
    LocalPendingSelection, LocalSelection, MarkedTextState, MovementResult, SelectionMode,
    ToBufferOffset, ToCharOffset, ToDisplayPoint, ToPoint,
};
use num_traits::{SaturatingSub, Zero};
use parking_lot::Mutex;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use serde::{Deserialize, Serialize};
use settings::Setting as _;
use snapshot::ViewSnapshot;
use soft_wrap::{ClampDirection, DisplayPointAndClampDirection};
use string_offset::{ByteOffset, CharOffset};
use vec1::{Vec1, vec1};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::color::ContrastingColor as _;
use warp_core::ui::color::contrast::MinimumAllowedContrast;
use warp_core::ui::theme::Fill;
use warp_editor::editor::NavigationKey;
use warp_errors::report_error;
use warp_util::user_input::UserInput;
use warpui::accessibility::{AccessibilityContent, ActionAccessibilityContent, WarpA11yRole};
use warpui::r#async::Timer;
use warpui::clipboard::ClipboardContent;
use warpui::elements::{DEFAULT_UI_LINE_HEIGHT_RATIO, Hoverable, MouseStateHandle};
use warpui::fonts::{Cache as FontCache, FamilyId, Properties};
use warpui::keymap::{EditableBinding, FixedBinding, PerPlatformKeystroke};
use warpui::platform::{Cursor, OperatingSystem};
use warpui::text::TextBuffer;
pub use warpui::text::point::Point;
use warpui::text::word_boundaries::WordBoundariesPolicy;
use warpui::text_layout::{ComputeBaselinePositionFn, TextStyle};
use warpui::windowing::WindowManager;
use warpui::{
    AppContext, BlurContext, CursorInfo, Element, Entity, EntityId, FocusContext, ModelAsRef,
    ModelContext, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext, WindowId,
    windowing,
};
/// The editor interfaces that we publicly expose to consumers.
/// This should be a very limited set; if you need to add something here,
/// you should carefully consider if it leaks the internal details of the editor.
pub use {
    element::{EditorDecoratorElements, EditorElement, TextColors},
    model::{
        Chars, DisplayPoint, EditOrigin, EditorSnapshot, InteractionState,
        LocalDrawableSelectionData, SelectAction, TextRun, TextStyleOperation,
    },
};

use self::model::{LocalSelections, Selection, UpdateBufferOption};

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_TAB_SIZE: usize = 4;

pub const SELECT_UP_ACTION_NAME: &str = "editor_view:select_up";
pub const SELECT_DOWN_ACTION_NAME: &str = "editor_view:select_down";

pub fn init(ctx: &mut AppContext) {
    use warpui::keymap::macros::*;

    EditorSettings::register(ctx);
    SelectionSettings::register(ctx);

    ctx.register_fixed_bindings(vec![
        // Below are default bindings that are similar to the behavior in all other text editors.
        // Those are not exposed to change, however, there may be editable bindings with different
        // bindings/triggers defined (so users can still adjust).
        FixedBinding::new(
            "escape",
            EditorAction::Escape,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "backspace",
            EditorAction::Backspace,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "enter",
            EditorAction::Enter,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new_per_platform(
            PerPlatformKeystroke {
                mac: "cmd-enter",
                linux_and_windows: "ctrl-shift-enter",
            },
            EditorAction::CmdEnter,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "numpadenter",
            EditorAction::Enter,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "delete",
            EditorAction::Delete,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "shift-enter",
            EditorAction::ShiftEnter,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "cmdorctrl-c",
            EditorAction::Copy,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "cmdorctrl-x",
            EditorAction::Cut,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "cmdorctrl-v",
            EditorAction::Paste,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "shift-tab",
            EditorAction::ShiftTab,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "tab",
            EditorAction::Tab,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new("up", EditorAction::Up, id!("EditorView") & !id!("IMEOpen")),
        FixedBinding::new(
            "down",
            EditorAction::Down,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "left",
            EditorAction::Left,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "right",
            EditorAction::Right,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        // NOTE: physical `home`/`end` keys are bound via the editable
        // `editor_view:home`/`editor_view:end` bindings below (linux/windows
        // map them to the visual-line action; macOS maps them to document
        // start/end). We intentionally do not register cross-platform
        // `home`/`end` FixedBindings here, as they would conflict with the
        // macOS document-navigation bindings.
        FixedBinding::new(
            "shift-up",
            EditorAction::SelectUp,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "shift-down",
            EditorAction::SelectDown,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "shift-left",
            EditorAction::SelectLeft,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "shift-right",
            EditorAction::SelectRight,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new_per_platform(
            PerPlatformKeystroke {
                mac: "shift-alt-left",
                linux_and_windows: "shift-ctrl-left",
            },
            EditorAction::SelectLeftByWord,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "shift-meta-left",
            EditorAction::SelectLeftByWord,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new_per_platform(
            PerPlatformKeystroke {
                mac: "shift-alt-right",
                linux_and_windows: "shift-ctrl-right",
            },
            EditorAction::SelectRightByWord,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "shift-meta-right",
            EditorAction::SelectRightByWord,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "shift-home",
            EditorAction::SelectToLineStart,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "shift-end",
            EditorAction::SelectToLineEnd,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "pageup",
            EditorAction::PageUp,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "pagedown",
            EditorAction::PageDown,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        // Some editable bindings currently have more than 1 action.
        // Below's the list of those.
        // TODO better way of exposing multiple keybindings to the user
        FixedBinding::new(
            "shift-backspace",
            EditorAction::Backspace,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "ctrl-enter",
            EditorAction::CtrlEnter,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "alt-enter",
            EditorAction::AltEnter,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "cmdorctrl-z",
            EditorAction::Undo,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "cmdorctrl-shift-Z",
            EditorAction::Redo,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "meta-f",
            EditorAction::MoveForwardOneWord,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "meta-right",
            EditorAction::MoveForwardOneWord,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "meta-b",
            EditorAction::MoveBackwardOneWord,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "meta-left",
            EditorAction::MoveBackwardOneWord,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "meta-backspace",
            EditorAction::CutWordLeft,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "meta-d",
            EditorAction::CutWordRight,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "meta-shift-{",
            EditorAction::MoveToParagraphStart,
            id!("EditorView") & !id!("IMEOpen"),
        ),
        FixedBinding::new(
            "meta-shift-}",
            EditorAction::MoveToParagraphEnd,
            id!("EditorView") & !id!("IMEOpen"),
        ),
    ]);

    // Emacs-style control bindings are native text-editing bindings on macOS only; elsewhere
    // the same keys are application shortcuts (ctrl-k, ctrl-f, ctrl-b, ...).
    if OperatingSystem::get().is_mac() {
        ctx.register_fixed_bindings([
            // A native character palette isn't supported on all platforms. The `ctrl-cmd-space`
            // binding is unique to Mac.
            FixedBinding::new(
                "ctrl-cmd-space",
                EditorAction::ShowCharacterPalette,
                id!("EditorView") & !id!("IMEOpen"),
            ),
            FixedBinding::new(
                "ctrl-y",
                EditorAction::Yank,
                id!("EditorView") & !id!("IMEOpen"),
            ),
            // This might seem like a no-op since `ctrl-right` changes desktops on Mac by default.
            // However, many Mac users have disabled the desktop change shortcut and expect this
            // binding to work.
            FixedBinding::new(
                "ctrl-right",
                EditorAction::MoveForwardOneWord,
                id!("EditorView") & !id!("IMEOpen"),
            ),
        ]);
    }

    ctx.register_editable_bindings([
        // Selections
        EditableBinding::new(
            "editor_view:select_left_by_word",
            "Select one word to the left",
            EditorAction::SelectLeftByWord,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_key_binding("shift-meta-B"),
        EditableBinding::new(
            "editor_view:select_right_by_word",
            "Select one word to the right",
            EditorAction::SelectRightByWord,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_key_binding("shift-meta-F"),
        EditableBinding::new(
            "editor_view:select_left",
            "Select one character to the left",
            EditorAction::SelectLeft,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("shift-ctrl-B"),
        EditableBinding::new(
            "editor_view:select_right",
            "Select one character to the right",
            EditorAction::SelectRight,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("shift-ctrl-F"),
        EditableBinding::new(SELECT_UP_ACTION_NAME, "Select up", EditorAction::SelectUp)
            .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
            .with_mac_key_binding("shift-ctrl-P"),
        EditableBinding::new(
            SELECT_DOWN_ACTION_NAME,
            "Select down",
            EditorAction::SelectDown,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("shift-ctrl-N"),
        EditableBinding::new(
            "editor_view:select_all",
            "Select all",
            EditorAction::SelectAll,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_key_binding("cmdorctrl-a"),
        EditableBinding::new(
            "editor:select_to_line_start",
            "Select to start of line",
            EditorAction::SelectToLineStart,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("shift-ctrl-A"),
        EditableBinding::new(
            "editor:select_to_line_end",
            "Select to end of line",
            EditorAction::SelectToLineEnd,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("shift-ctrl-E"),
        EditableBinding::new(
            "editor_view:clear_and_copy_lines",
            "Copy and clear selected lines",
            EditorAction::ClearAndCopyLines,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("ctrl-u"),
        EditableBinding::new(
            "editor_view:add_next_occurrence",
            "Add selection for next occurrence",
            EditorAction::AddNextOccurrence,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("ctrl-g"),
        // `shift-end` is registered on all platforms for this action.
        EditableBinding::new(
            "editor_view:select_to_line_end",
            "Select To Line End",
            EditorAction::SelectToLineEnd,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("cmd-shift-right"),
        // `shift-home` is registered on all platforms for this action.
        EditableBinding::new(
            "editor_view:select_to_line_start",
            "Select To Line Start",
            EditorAction::SelectToLineStart,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("cmd-shift-left"),
        // Navigation
        EditableBinding::new("editor_view:up", "Move cursor up", EditorAction::Up)
            .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
            .with_mac_key_binding("ctrl-p"),
        EditableBinding::new("editor_view:down", "Move cursor down", EditorAction::Down)
            .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
            .with_mac_key_binding("ctrl-n"),
        EditableBinding::new("editor_view:left", "Move cursor left", EditorAction::Left)
            .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
            .with_mac_key_binding("ctrl-b"),
        EditableBinding::new(
            "editor_view:right",
            "Move cursor right",
            EditorAction::Right,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("ctrl-f"),
        EditableBinding::new(
            "editor_view:move_to_line_start",
            "Move to start of line",
            EditorAction::MoveToLineStart,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("ctrl-a"),
        EditableBinding::new(
            "editor_view:move_to_line_end",
            "Move to end of line",
            EditorAction::MoveToLineEnd,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("ctrl-e"),
        // Match the behavior of both VSCode and Intellij by using `cmd-left/right` on Mac and
        // `home/end` on Windows and Linux. See https://www.jetbrains.com/help/idea/reference-keymap-win-default.html#caret_navigation.
        EditableBinding::new(
            "editor_view:home",
            "Home",
            EditorAction::MoveToVisualLineStart,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("cmd-left")
        .with_linux_or_windows_key_binding("home"),
        EditableBinding::new("editor_view:end", "End", EditorAction::MoveToVisualLineEnd)
            .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
            .with_mac_key_binding("cmd-right")
            .with_linux_or_windows_key_binding("end"),
        EditableBinding::new(
            "editor_view:cmd_down",
            "Move cursor to the bottom",
            EditorAction::CmdDown,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        // Match the behavior of VSCode, see https://code.visualstudio.com/docs/getstarted/keybindings#_basic-editing.
        .with_mac_key_binding("cmd-down")
        .with_linux_or_windows_key_binding("ctrl-end"),
        EditableBinding::new(
            "editor_view:cmd_up",
            "Move cursor to the top",
            EditorAction::CmdUp,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        // Match the behavior of VSCode, see https://code.visualstudio.com/docs/getstarted/keybindings#_basic-editing.
        .with_mac_key_binding("cmd-up")
        .with_linux_or_windows_key_binding("ctrl-home"),
        EditableBinding::new(
            "editor_view:move_to_and_select_buffer_start",
            "Select and move to the top",
            EditorAction::MoveToAndSelectBufferStart,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("cmd-shift-up")
        .with_linux_or_windows_key_binding("ctrl-shift-home"),
        EditableBinding::new(
            "editor_view:move_to_and_select_buffer_end",
            "Select and move to the bottom",
            EditorAction::MoveToAndSelectBufferEnd,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("cmd-shift-down")
        .with_linux_or_windows_key_binding("ctrl-shift-end"),
        EditableBinding::new(
            "editor_view:move_to_paragraph_start",
            "Move to the start of the paragraph",
            EditorAction::MoveToParagraphStart,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_key_binding("meta-a"),
        EditableBinding::new(
            "editor_view:move_to_paragraph_end",
            "Move to the end of the paragraph",
            EditorAction::MoveToParagraphEnd,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_key_binding("meta-e"),
        EditableBinding::new(
            "editor_view:move_to_buffer_start",
            "Move to the start of the buffer",
            EditorAction::MoveToBufferStart,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_key_binding("meta-shift-<"),
        EditableBinding::new(
            "editor_view:move_to_buffer_end",
            "Move to the end of the buffer",
            EditorAction::MoveToBufferEnd,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_key_binding("meta-shift->"),
        // On macOS, the physical Home/End keys jump to the start/end of the
        // document (matching the macOS convention), distinct from `cmd-left`/
        // `cmd-right` which move to the visual-line start/end. On Linux/Windows
        // the Home/End keys remain bound to the visual-line action above.
        EditableBinding::new(
            "editor_view:move_to_buffer_start",
            "Move to the start of the buffer",
            EditorAction::MoveToBufferStart,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("home"),
        EditableBinding::new(
            "editor_view:move_to_buffer_end",
            "Move to the end of the buffer",
            EditorAction::MoveToBufferEnd,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("end"),
        // Buffer modifications
        EditableBinding::new(
            "editor_view:backspace",
            "Remove the previous character",
            EditorAction::Backspace,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("ctrl-h"),
        EditableBinding::new(
            "editor_view:cut_word_left",
            "Cut word left",
            EditorAction::CutWordLeft,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("ctrl-w"),
        EditableBinding::new(
            "editor:delete_word_left",
            "Delete word left",
            EditorAction::DeleteWordLeft,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("alt-backspace")
        .with_linux_or_windows_key_binding("ctrl-backspace"),
        EditableBinding::new(
            "editor_view:cut_word_right",
            "Cut word right",
            EditorAction::CutWordRight,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("alt-d"),
        EditableBinding::new("editor_view:delete", "Delete", EditorAction::Delete)
            .with_context_predicate(id!("EditorView") & !id!("EditorView_SingleCursorBufferEnd"))
            .with_mac_key_binding("ctrl-d"),
        EditableBinding::new(
            "editor:delete_word_right",
            "Delete word right",
            EditorAction::DeleteWordRight,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("alt-delete")
        .with_linux_or_windows_key_binding("ctrl-delete"),
        EditableBinding::new(
            "editor_view:clear_lines",
            "Clear selected lines",
            EditorAction::ClearAndCopyLines,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("cmd-shift-K"),
        EditableBinding::new(
            "editor_view:cut_all_right",
            "Cut all right",
            EditorAction::CutAllRight,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("ctrl-k"),
        EditableBinding::new(
            "editor_view:delete_all_right",
            "Delete all right",
            EditorAction::DeleteAllRight,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        // VSCode only binds a default binding on Mac, see https://github.com/microsoft/vscode/blob/ceda6cc4856841f1550a60327a3eaf3a1d0c306a/src/vs/editor/contrib/linesOperations/browser/linesOperations.ts#L657.
        .with_mac_key_binding("cmd-delete"),
        EditableBinding::new(
            "editor_view:delete_all_left",
            "Delete all left",
            EditorAction::DeleteAllLeft,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("cmd-backspace"),
        EditableBinding::new(
            "editor_view:insert_newline",
            "Insert newline",
            EditorAction::Newline,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("ctrl-j"),
        EditableBinding::new(
            "editor_view:move_backward_one_word",
            "Move Backward One Word",
            EditorAction::MoveBackwardOneWord,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("alt-left")
        .with_linux_or_windows_key_binding("ctrl-left"),
        EditableBinding::new(
            "editor_view:move_forward_one_word",
            "Move Forward One Word",
            EditorAction::MoveForwardOneWord,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("alt-right")
        .with_linux_or_windows_key_binding("ctrl-right"),
        EditableBinding::new(
            "editor_view:move_backward_one_subword",
            "Move Backward One Subword",
            EditorAction::MoveBackwardOneSubword,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        // Only assign a default keybinding for subword navigation on Mac, this is also what VSCode
        // does: https://github.com/microsoft/vscode/blob/e08f57208f087abed82852b24b5f4937357a95c1/src/vs/editor/contrib/wordPartOperations/browser/wordPartOperations.ts#L85.
        .with_mac_key_binding("ctrl-alt-left"),
        EditableBinding::new(
            "editor_view:move_forward_one_subword",
            "Move Forward One Subword",
            EditorAction::MoveForwardOneSubword,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("ctrl-alt-right"),
        EditableBinding::new(
            "editor_view:select_left_by_subword",
            "Select one subword to the left",
            EditorAction::SelectLeftBySubword,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("ctrl-alt-shift-left"),
        EditableBinding::new(
            "editor_view:select_right_by_subword",
            "Select one subword to the right",
            EditorAction::SelectRightBySubword,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_mac_key_binding("ctrl-alt-shift-right"),
        EditableBinding::new(
            "editor_view:add_cursor_above",
            "Add cursor above",
            EditorAction::AddCursorAbove,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_key_binding("ctrl-shift-up"),
        EditableBinding::new(
            "editor_view:add_cursor_below",
            "Add cursor below",
            EditorAction::AddCursorBelow,
        )
        .with_context_predicate(id!("EditorView") & !id!("IMEOpen"))
        .with_key_binding("ctrl-shift-down"),
    ]);
}

/// Actions that can be performed on an [`EditorView`].
#[derive(Debug)]
pub enum EditorAction {
    Scroll(Vector2F),
    Select(SelectAction),
    UserInsert(UserInput<String>),
    SetMarkedText {
        marked_text: UserInput<String>,
        selected_range: Range<usize>,
    },
    ClearMarkedText,
    ImeCommit(UserInput<String>),
    Tab,
    ShiftTab,
    Copy,
    Cut,
    Paste,
    MiddleClickPaste,
    Yank,
    Newline,
    CtrlEnter,
    ShiftEnter,
    AltEnter,
    Enter,
    CutWordLeft,
    CutWordRight,
    DeleteWordLeft,
    DeleteWordRight,
    CutAllRight,
    CutAllLeft,
    DeleteAllRight,
    DeleteAllLeft,
    Delete,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    MoveToVisualLineStart,
    MoveToVisualLineEnd,
    PageUp,
    PageDown,
    CmdUp,
    CmdDown,
    ClearAndCopyLines,
    MoveToLineStart,
    MoveToParagraphStart,
    MoveToBufferStart,
    SelectToLineStart,
    MoveToLineEnd,
    MoveToParagraphEnd,
    MoveToBufferEnd,
    SelectToLineEnd,
    MoveToAndSelectBufferStart,
    MoveToAndSelectBufferEnd,
    MoveForwardOneWord,
    MoveBackwardOneWord,
    MoveForwardOneSubword,
    MoveBackwardOneSubword,
    SelectUp,
    SelectDown,
    SelectLeft,
    SelectRight,
    SelectWord(DisplayPoint),
    SelectLine(DisplayPoint),
    SelectAll,
    SelectRightByWord,
    SelectLeftByWord,
    SelectRightBySubword,
    SelectLeftBySubword,
    AddNextOccurrence,
    Undo,
    Redo,
    Focus,
    ClearParentSelections,
    AddCursorAbove,
    AddCursorBelow,
    CmdEnter,
    ShowCharacterPalette,
}

impl EditorAction {
    fn should_report_active_cursor_position_updated(&self) -> bool {
        !matches!(
            self,
            EditorAction::Scroll(_)
                | EditorAction::Copy
                | EditorAction::ShowCharacterPalette
                | EditorAction::Select(_)
        )
    }

    fn is_new_selection(&self) -> bool {
        matches!(
            self,
            EditorAction::Select(
                SelectAction::Update { .. } | SelectAction::Extend { .. } | SelectAction::End,
            ) | EditorAction::SelectToLineStart
                | EditorAction::SelectToLineEnd
                | EditorAction::SelectUp
                | EditorAction::SelectDown
                | EditorAction::SelectLeft
                | EditorAction::SelectRight
                | EditorAction::SelectWord(_)
                | EditorAction::SelectLine(_)
                | EditorAction::SelectAll
                | EditorAction::SelectRightByWord
                | EditorAction::SelectLeftByWord
                | EditorAction::SelectRightBySubword
                | EditorAction::SelectLeftBySubword
        )
    }
}

/// Configuration for `insert_internal`
/// Whether we want the inserted text to be placed in a selection.
pub enum SelectionInsertion {
    Yes,
    No,
}

#[derive(Debug)]
enum CutDirection {
    Right,
    Left,
}

/// This type is used specifically used to add a cursor above or below.
/// For that reason, when we move the cursor above or below the behavior is
/// different from normal cursor movement in that moving above in the top row
/// will return None instead of the first column and similarly moving below
/// in the bottom row will return None instead of the max point.
enum NewCursorDirection {
    Up,
    Down,
}

struct NewCursorUpOrDownResult {
    point_and_clamp_direction: DisplayPointAndClampDirection,
    goal_column: u32,
}

impl From<MovementResult> for NewCursorUpOrDownResult {
    fn from(result: MovementResult) -> Self {
        NewCursorUpOrDownResult {
            point_and_clamp_direction: result.point_and_clamp_direction,
            goal_column: result.goal_column,
        }
    }
}

impl NewCursorDirection {
    fn move_cursor(
        &self,
        map: &DisplayMap,
        point: DisplayPoint,
        goal_column: Option<u32>,
        clamp_direction: ClampDirection,
    ) -> Option<NewCursorUpOrDownResult> {
        match self {
            NewCursorDirection::Up => match map.up(point, goal_column, clamp_direction) {
                Ok(result) => {
                    if result.is_same_row {
                        None
                    } else {
                        Some(result.into())
                    }
                }
                Err(err) => {
                    report_error!(err.context("Error calling map#up"));
                    None
                }
            },
            NewCursorDirection::Down => match map.down(point, goal_column, clamp_direction) {
                Ok(result) => {
                    if result.is_same_row {
                        None
                    } else {
                        Some(result.into())
                    }
                }
                Err(err) => {
                    report_error!(err.context("Error calling map#down"));
                    None
                }
            },
        }
    }
}

/// Actions that can be performed on a plain text editor view.
#[derive(PartialEq, Eq, PartialOrd, Clone, Copy)]
pub enum PlainTextEditorViewAction {
    InsertChar,
    CursorChanged,
    Yank,
    CutWordLeft,
    ClearBuffer,
    DeleteWordLeft,
    DeleteWordRight,
    CutWordRight,
    Tab,
    Indent,
    Unindent,
    InsertSelectedText,
    SystemInsert,
    Space,
    NewLine,
    Backspace,
    Delete,
    CutAll,
    DeleteAll,
    ClearAndCopyLines,
    ClearLines,
    ReplaceBuffer,
    Paste,
    UpdateMarkedText,
}

impl PlainTextEditorViewAction {
    fn from_inserted_str(c: &str) -> Self {
        if c == " " {
            Self::Space
        } else {
            // TODO: we should probably rename this to "insert text" based on
            // how it's used.
            Self::InsertChar
        }
    }
}

#[derive(Clone, Debug)]
pub enum ValidInputType {
    All,
    /// Attempts to parse input as a `u16`.
    PositiveInteger,
    NoSpaces,
}

/// Possible action that could be triggered by enter (including modified enter).
#[derive(Clone)]
pub enum EnterAction {
    /// Emit the event to the parent level.
    Emit,
    /// Insert a new line for this enter action.
    InsertNewLineIfMultiLine,
}

/// Settings for different enter keystrokes.
#[derive(Clone)]
pub struct EnterSettings {
    pub shift_enter: EnterAction,
    pub enter: EnterAction,
    pub alt_enter: EnterAction,
    pub ctrl_enter: EnterAction,
}

impl Default for EnterSettings {
    fn default() -> Self {
        Self {
            shift_enter: EnterAction::InsertNewLineIfMultiLine,
            enter: EnterAction::Emit,
            alt_enter: EnterAction::InsertNewLineIfMultiLine,
            ctrl_enter: EnterAction::InsertNewLineIfMultiLine,
        }
    }
}

#[derive(Clone, Copy)]
pub enum PropagateAndNoOpNavigationKeys {
    Always,
    /// Propagate the up and down arrow keys only when the cursor is at the boundary --
    /// so we will emit EditorEvent::Navigate(Up) when the cursor is at the first row and
    /// emit EditorEvent::Navigate(Down) when the cursor is at the last row.
    AtBoundary,
    Never,
}

#[derive(Clone, Copy, Default)]
pub enum PropagateHorizontalNavigationKeys {
    /// Always propagate horizontal navigation keys to parent
    Always,
    /// Only propagate when cursor is at buffer boundaries (start for left, end for right)
    AtBoundary,
    /// Never propagate - always handle within editor
    #[default]
    Never,
}

/// Options for how the editor displays text.
///
/// Each is an `Option` - if `None`, the editor will use the theme default. If a text option is set,
/// the parent view is responsible for keeping it in sync with appearance changes (if applicable).
#[derive(Default, Clone)]
pub struct TextOptions {
    /// If font size is None, we use the settings monospace font size, otherwise we use the given one.
    pub font_size_override: Option<f32>,
    /// Font family to use when rendering the editor. If `None`, the monospace font is used.
    pub font_family_override: Option<FamilyId>,
    /// Default font properties for editor text.
    pub font_properties_override: Option<Properties>,
    /// Colors to use when rendering editor text. If `None`, the theme's text colors are used.
    pub text_colors_override: Option<TextColors>,
}

impl TextOptions {
    /// Create `TextOptions` that use the UI font size. All other overrides have their default values.
    pub fn ui_font_size(appearance: &Appearance) -> Self {
        Self {
            font_size_override: Some(appearance.ui_font_size()),
            font_properties_override: Some(Properties::default()),
            ..Default::default()
        }
    }

    /// Create `TextOptions` to use the UI font family at the given size. All other overrides
    /// have their default value.
    pub fn ui_text(size: Option<f32>, appearance: &Appearance) -> Self {
        Self {
            font_size_override: size,
            font_family_override: Some(appearance.ui_font_family()),
            font_properties_override: Some(Properties::default()),
            ..Default::default()
        }
    }
}

/// Colors to use when rendering cursors and selections in the editor.
#[derive(Clone, Copy)]
pub struct CursorColors {
    /// The color to use for the cursor.
    pub cursor: Fill,
    /// The color to use as the background of selections.
    pub selection: Fill,
}

/// Type alias for the function that returns the Editor Decorator Elements. The first `String` parameter is for the editor buffer contents.
type RenderDecoratorElementsFn = Box<dyn Fn(&AppContext) -> EditorDecoratorElements>;

/// Type alias for a closure that allows parent views to add flags to the EditorView's keymap context.
/// The closure takes the context by mutable reference and can insert additional flags.
pub type KeymapContextModifierFn = Box<dyn Fn(&mut warpui::keymap::Context, &AppContext)>;

/// Enum to choose between different methods of computing the baseline offset for text.
#[derive(Clone, Debug)]
pub enum BaselinePositionComputationMethod {
    /// Calculated "grid-style", where we use solely font metrics (ascent, descent, etc.) to
    /// compute the baseline offset.
    Grid,
    /// The default computation method, which is calculated "line-style", where we use the standard baseline offset
    /// computation for a Line (ultimately uses TOP_BOTTOM_RATIO).
    Default,
}

/// Interface for picking different options for the editor's behavior.
pub struct EditorOptions {
    pub text: TextOptions,
    pub cursor_colors_fn: CursorColorsFn,
    pub enter_settings: EnterSettings,
    pub propagate_and_no_op_vertical_navigation_keys: PropagateAndNoOpNavigationKeys,
    pub propagate_horizontal_navigation_keys: PropagateHorizontalNavigationKeys,
    pub autogrow: bool,
    pub single_line: bool,
    pub use_settings_line_height_ratio: bool,
    pub autocomplete_symbols: bool,
    pub soft_wrap: bool,
    pub placeholder_soft_wrap: bool,
    /// Closures that should return a top section, left notch and right notch elements, if we want to paint them
    /// (above, top-left and top-right of EditorElement).
    pub render_decorator_elements: Option<RenderDecoratorElementsFn>,
    pub select_all_on_focus: bool,
    pub clear_selections_on_blur: bool,
    pub max_buffer_len: Option<usize>,
    pub valid_input_type: ValidInputType,
    /// Whether to use a Grid-style baseline position computation (using font metrics), or the standard
    /// baseline position computation for a Line (which uses TOP_BOTTOM_RATIO).
    pub baseline_position_computation_method: BaselinePositionComputationMethod,
    pub middle_click_paste: bool,
    /// If true, the user's [`CursorDisplayType`] will be respected.
    pub allow_user_cursor_preference: bool,
    pub convert_newline_to_space: bool,
    /// If true, this editor will delegate handling of paste events to its parent instead of
    /// inserting clipboard contents directly.
    pub delegate_paste_handling: bool,
    /// If true, this is treated as a password field:
    /// * Text is rendered as dots instead of the actual characters
    /// * Copying is disabled (but paste still works)
    pub is_password: bool,
    /// Optional closure that allows parent views to add flags to the EditorView's keymap context.
    /// This is called during `keymap_context()` and can insert additional flags into the context.
    pub keymap_context_modifier: Option<KeymapContextModifierFn>,
}

impl Default for EditorOptions {
    fn default() -> Self {
        Self {
            text: Default::default(),
            enter_settings: Default::default(),
            cursor_colors_fn: Box::new(default_cursor_colors),
            propagate_and_no_op_vertical_navigation_keys: PropagateAndNoOpNavigationKeys::Never,
            propagate_horizontal_navigation_keys: PropagateHorizontalNavigationKeys::Never,
            autogrow: false,
            single_line: false,
            use_settings_line_height_ratio: false,
            autocomplete_symbols: false,
            soft_wrap: false,
            placeholder_soft_wrap: false,
            render_decorator_elements: None,
            select_all_on_focus: false,
            clear_selections_on_blur: false,
            max_buffer_len: None,
            valid_input_type: ValidInputType::All,
            baseline_position_computation_method: BaselinePositionComputationMethod::Default,
            middle_click_paste: true,
            allow_user_cursor_preference: false,
            convert_newline_to_space: false,
            delegate_paste_handling: false,
            is_password: false,
            keymap_context_modifier: None,
        }
    }
}

impl From<SingleLineEditorOptions> for EditorOptions {
    fn from(options: SingleLineEditorOptions) -> Self {
        Self {
            text: options.text,
            enter_settings: options.enter_settings,
            cursor_colors_fn: Box::new(default_cursor_colors),
            propagate_and_no_op_vertical_navigation_keys: options
                .propagate_and_no_op_vertical_navigation_keys,
            propagate_horizontal_navigation_keys: options.propagate_horizontal_navigation_keys,
            autogrow: false,
            single_line: true,
            use_settings_line_height_ratio: options.use_settings_line_height_ratio,
            autocomplete_symbols: options.autocomplete_symbols,
            soft_wrap: options.soft_wrap,
            placeholder_soft_wrap: options.placeholder_soft_wrap,
            render_decorator_elements: None,
            select_all_on_focus: options.select_all_on_focus,
            clear_selections_on_blur: options.clear_selections_on_blur,
            max_buffer_len: options.max_buffer_len,
            valid_input_type: options.valid_input_type,
            baseline_position_computation_method: options.baseline_position_computation_method,
            middle_click_paste: options.middle_click_paste,
            allow_user_cursor_preference: options.allow_user_cursor_preference,
            convert_newline_to_space: options.convert_newline_to_space,
            delegate_paste_handling: false,
            is_password: options.is_password,
            keymap_context_modifier: None,
        }
    }
}

/// Interface for picking different options for the editor's behavior.
///
/// There are some fields that are not applicable to single-line editors which
/// are absent from this struct and have the behavior turned off in the `From`
/// implementation that convert a `SingleLineEditorOptions` to an `EditorOptions`.
#[derive(Clone)]
pub struct SingleLineEditorOptions {
    pub text: TextOptions,
    pub enter_settings: EnterSettings,
    pub propagate_and_no_op_vertical_navigation_keys: PropagateAndNoOpNavigationKeys,
    pub propagate_horizontal_navigation_keys: PropagateHorizontalNavigationKeys,
    pub use_settings_line_height_ratio: bool,
    pub autocomplete_symbols: bool,
    pub soft_wrap: bool,
    pub placeholder_soft_wrap: bool,
    pub select_all_on_focus: bool,
    pub clear_selections_on_blur: bool,
    pub max_buffer_len: Option<usize>,
    pub valid_input_type: ValidInputType,
    /// Whether to use a Grid-style baseline offset computation (using font metrics), or the standard
    /// baseline offset computation for a Line (which uses TOP_BOTTOM_RATIO).
    pub baseline_position_computation_method: BaselinePositionComputationMethod,
    pub middle_click_paste: bool,
    /// If true, the user's [`CursorDisplayType`] will be respected.
    pub allow_user_cursor_preference: bool,
    pub convert_newline_to_space: bool,
    pub is_password: bool,
}

impl Default for SingleLineEditorOptions {
    fn default() -> Self {
        Self {
            text: Default::default(),
            enter_settings: Default::default(),
            propagate_and_no_op_vertical_navigation_keys: PropagateAndNoOpNavigationKeys::Never,
            propagate_horizontal_navigation_keys: PropagateHorizontalNavigationKeys::Never,
            use_settings_line_height_ratio: false,
            autocomplete_symbols: false,
            soft_wrap: false,
            placeholder_soft_wrap: false,
            select_all_on_focus: false,
            clear_selections_on_blur: false,
            max_buffer_len: None,
            valid_input_type: ValidInputType::All,
            baseline_position_computation_method: BaselinePositionComputationMethod::Default,
            middle_click_paste: true,
            allow_user_cursor_preference: false,
            convert_newline_to_space: true,
            is_password: false,
        }
    }
}

pub type CursorColorsFn = Box<dyn Fn(&AppContext) -> CursorColors>;

/// Returns theme-based cursor and selection colors.
pub fn default_cursor_colors(ctx: &AppContext) -> CursorColors {
    let theme = Appearance::as_ref(ctx).theme();
    CursorColors {
        cursor: theme
            .cursor()
            .on_background(theme.background(), MinimumAllowedContrast::Text),
        selection: theme.text_selection_color(),
    }
}

pub struct EditorView {
    view_id: EntityId,
    editor_model: ModelHandle<EditorModel>,
    scroll_position: Arc<Mutex<Vector2F>>,
    autoscroll_requested: Arc<Mutex<bool>>,
    windowing_state_handle: ModelHandle<WindowManager>,

    text_options: TextOptions,
    use_settings_line_height_ratio: bool,
    focused: bool,
    get_cursor_colors_fn: CursorColorsFn,
    cursors_visible: bool,
    blink_epoch: usize,
    single_line: bool,
    /// A cache for text that is cut within the input using ctrl-k/u. This text is not copied to clipboard.
    internal_clipboard: String,
    /// Sets whether to propagate the vertical navigation keys (up/down) action to parent view.
    propagate_vertical_navigation_keys: PropagateAndNoOpNavigationKeys,
    /// Sets whether to propagate the horizontal navigation keys (left/right) action to parent view.
    propagate_horizontal_navigation_keys: PropagateHorizontalNavigationKeys,
    autogrow: bool,
    /// If true, defers to the user's settings for whether to autocomplete
    /// typed symbols.
    autocomplete_symbols_allowed: bool,
    /// A cached copy of autocomplete_symbols from Settings (to avoid performing
    /// a settings read on every typed character).
    autocomplete_symbols_setting: bool,
    /// A cached copy of cursor_display_type from Settings (to avoid performing
    /// a settings read on every typed character).
    /// If `None`, we do not wish to respect the user's cursor display preference.
    cursor_display_override: Option<CursorDisplayType>,
    window_id: WindowId,

    /// Map from prefix to placeholder text.
    /// Empty string prefix "" is the default placeholder (shown when buffer is empty).
    placeholder_texts: Arc<HashMap<String, String>>,
    hover_handle: MouseStateHandle,
    enter_settings: EnterSettings,
    soft_wrap: bool,
    placeholder_soft_wrap: bool,
    /// What the starting text of the view was. Helpful for determining if the editor is
    /// "dirty", i.e., a user has changed the contents from what is persisted elsewhere.
    base_buffer_text: String,

    /// Optionally, we render left/right decorator elements, using the given closure.
    render_decorator_elements: Option<RenderDecoratorElementsFn>,

    /// Indicates whether or not we want to select the entire editor contents when we focus the editor view.
    select_all_on_focus: bool,
    /// Indicates whether or not we want to clear selections in the editor on blur.
    clear_selections_on_blur: bool,

    /// A map from position ID to buffer point.
    /// Each entry in this map corresponds to a buffer position that will be cached
    /// via the position cache. The cached position will be the top-left of the bounding
    /// box for the corresponding point.
    ///
    /// See [`Self::cache_buffer_point`] for more details.
    // TODO: consider wrapping this in a reference-counted variable
    // if we end up using this API more. Currently, we clone this map
    // for every render, which is inefficient.
    cached_buffer_points: HashMap<Cow<'static, str>, Point>,

    /// Whether to use a Grid-style baseline position computation (using font metrics), or the standard
    /// baseline position computation for a Line (which uses TOP_BOTTOM_RATIO).
    pub baseline_position_computation_method: BaselinePositionComputationMethod,

    /// Whether or not this editor supports middle-click-paste.
    /// This can be configured via [`EditorOptions::middle_click_paste`].
    ///
    /// The main use-case for configuring this is to set it to `false`
    /// to avoid a double-paste bug if the [`EditorView`] has a parent
    /// that already handles middle-click-paste.
    middle_click_paste: bool,

    /// If true, newlines will be converts to space on paste.
    /// This is useful for maintaining the single-line constraint
    convert_newline_to_space: bool,

    /// Whether this editor should delegate handling of paste events to its parent.
    delegate_paste_handling: bool,

    is_password: bool,

    /// Optional closure that allows parent views to add flags to this editor's keymap context.
    keymap_context_modifier: Option<KeymapContextModifierFn>,
}

pub(super) struct ScrollState {
    pub scroll_position: Arc<Mutex<Vector2F>>,
    pub autoscroll_requested: Arc<Mutex<bool>>,
}

impl ScrollState {
    pub(super) fn scroll_position(&self) -> Vector2F {
        *self.scroll_position.lock()
    }
}

impl From<&EditorView> for ScrollState {
    fn from(view: &EditorView) -> Self {
        ScrollState {
            scroll_position: view.scroll_position.clone(),
            autoscroll_requested: view.autoscroll_requested.clone(),
        }
    }
}

impl EditorView {
    fn snapshot(&self, ctx: &AppContext) -> ViewSnapshot {
        let font_cache = ctx.font_cache();
        let appearance = Appearance::as_ref(ctx);
        ViewSnapshot {
            view_id: self.view_id,
            is_focused: self.is_focused(),
            editor_model: self.editor_model.clone(),

            can_select: self.can_select(ctx),

            font_size: self.font_size(appearance),
            font_family: self.font_family(appearance),
            placeholder_font_family: self.placeholder_font_family(appearance),
            font_properties: self.font_properties(appearance),
            line_height: self.line_height(font_cache, appearance),
            line_height_ratio: self.line_height_ratio(appearance),
            em_width: self.em_width(font_cache, appearance),

            autogrow: self.autogrow,
            is_empty: self.is_empty(ctx),

            placeholder_texts: self.placeholder_texts.clone(),

            cached_buffer_points: self.cached_buffer_points.clone(),

            baseline_position_computation_method: self.baseline_position_computation_method.clone(),
        }
    }

    fn clipboard_content(&mut self, ctx: &mut ViewContext<Self>) -> String {
        let content = ctx.clipboard().read();
        self.clipboard_text_content(content)
    }

    pub fn clipboard_text_content(&self, content: ClipboardContent) -> String {
        clipboard_text(content, self.convert_newline_to_space)
    }

    fn middle_click_paste_content(&mut self, ctx: &mut ViewContext<Self>) -> Option<String> {
        let content = SelectionSettings::handle(ctx).update(ctx, |selection, ctx| {
            selection.read_for_middle_click_paste(ctx)
        });
        content.map(|content| self.clipboard_text_content(content))
    }

    /// Creates a single-line [`EditorView`] with an empty buffer
    /// and with behaviour specified by `options`.
    pub fn single_line(options: SingleLineEditorOptions, ctx: &mut ViewContext<Self>) -> Self {
        let options: EditorOptions = options.into();
        Self::new(options, ctx)
    }

    /// Creates an [`EditorView`] with an empty buffer
    /// and with behaviour specified by `options`.
    pub fn new(options: EditorOptions, ctx: &mut ViewContext<Self>) -> Self {
        Self::new_internal("", options, ctx)
    }

    fn new_internal(
        base_text: impl Into<String>,
        options: EditorOptions,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let base_text = base_text.into();
        let windowing_state_handle = WindowManager::handle(ctx);
        ctx.subscribe_to_model(
            &windowing_state_handle,
            |editor, _handle, evt, ctx| match evt {
                windowing::StateEvent::ValueChanged { current, previous } => {
                    editor.handle_windowing_state_event((current, previous), ctx);
                }
            },
        );

        let editor_model = ctx.add_model(|ctx| {
            EditorModel::new(
                base_text.clone(),
                DEFAULT_TAB_SIZE,
                options.max_buffer_len,
                options.valid_input_type,
                ctx,
            )
        });
        ctx.subscribe_to_model(&editor_model, Self::handle_model_event);

        let editor_settings_handle = &EditorSettings::handle(ctx);
        ctx.subscribe_to_model(editor_settings_handle, Self::handle_editor_settings_update);

        let cursor_display_override = if options.allow_user_cursor_preference {
            Some(*editor_settings_handle.as_ref(ctx).cursor_display_type)
        } else {
            None
        };

        Self {
            view_id: ctx.view_id(),
            editor_model,
            scroll_position: Arc::new(Mutex::new(Vector2F::zero())),
            autoscroll_requested: Arc::new(Mutex::new(false)),
            text_options: options.text,
            use_settings_line_height_ratio: options.use_settings_line_height_ratio,
            windowing_state_handle,
            focused: false,
            cursors_visible: false,
            blink_epoch: 0,
            single_line: options.single_line,
            internal_clipboard: "".to_string(),
            propagate_vertical_navigation_keys: options
                .propagate_and_no_op_vertical_navigation_keys,
            propagate_horizontal_navigation_keys: options.propagate_horizontal_navigation_keys,
            autogrow: options.autogrow,
            window_id: ctx.window_id(),
            autocomplete_symbols_allowed: options.autocomplete_symbols,
            autocomplete_symbols_setting: *editor_settings_handle.as_ref(ctx).autocomplete_symbols,
            cursor_display_override,
            placeholder_texts: Arc::new(HashMap::new()),
            hover_handle: Default::default(),
            enter_settings: options.enter_settings,
            soft_wrap: options.soft_wrap,
            placeholder_soft_wrap: options.placeholder_soft_wrap,
            base_buffer_text: base_text,
            render_decorator_elements: options.render_decorator_elements,
            select_all_on_focus: options.select_all_on_focus,
            clear_selections_on_blur: options.clear_selections_on_blur,
            get_cursor_colors_fn: options.cursor_colors_fn,
            cached_buffer_points: Default::default(),
            baseline_position_computation_method: options.baseline_position_computation_method,
            middle_click_paste: options.middle_click_paste,
            convert_newline_to_space: options.convert_newline_to_space,
            delegate_paste_handling: options.delegate_paste_handling,
            is_password: options.is_password,
            keymap_context_modifier: options.keymap_context_modifier,
        }
    }

    /// A helper function to make arbitrary edits more ergonomic.
    fn edit<C, F, U, B>(&mut self, ctx: &mut ViewContext<Self>, edits: Edits<C, F, U, B>)
    where
        C: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
        F: FnOnce(&EditorModel, &mut ModelContext<EditorModel>),
        U: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
        B: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
    {
        self.editor_model
            .update(ctx, |model, ctx| model.edit(ctx, edits))
    }

    /// A helper function to make bare selection changes more ergonomic.
    fn change_selections<C>(&mut self, ctx: &mut ViewContext<Self>, change_selections: C)
    where
        C: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
    {
        self.edit(ctx, Edits::new().with_change_selections(change_selections))
    }

    /// Sets the font family to use when rendering editor text to `font_family_id`.
    pub fn set_font_family(&mut self, font_family_id: FamilyId, ctx: &mut ViewContext<Self>) {
        self.text_options.font_family_override = Some(font_family_id);
        ctx.notify();
    }

    pub fn set_font_size(&mut self, new_font_size: f32, ctx: &mut ViewContext<Self>) {
        self.text_options.font_size_override = Some(new_font_size);
        ctx.notify();
    }

    pub fn set_text_colors(&mut self, text_colors: TextColors, ctx: &mut ViewContext<Self>) {
        self.text_options.text_colors_override = Some(text_colors);
        ctx.notify();
    }

    fn handle_windowing_state_event(
        &mut self,
        (current, previous): (&windowing::State, &windowing::State),
        ctx: &mut ViewContext<Self>,
    ) {
        let prev_cursor_active = self.focused && previous.active_window == Some(ctx.window_id());
        let curr_cursor_active = self.focused && current.active_window == Some(ctx.window_id());

        // Check if the windowing state update caused the state of our cursor to change
        match (prev_cursor_active, curr_cursor_active) {
            (false, true) => {
                // The windowing state update caused the cursor to go from inactive to active
                // Immediately show the cursor and update the blink timer
                self.reset_cursor_blink_timer(ctx);
            }
            (true, false) => {
                // The windowing state update caused the cursor to go from active to inactive
                // Notify the UI framework so that it is hidden on the next render
                ctx.notify();
            }
            _ => {
                // Otherwise, there was no change, so we don't need to call notify
            }
        }
    }

    /// Caches the given point via the position cache.
    /// The position will be cached with the ID `editor_{editor_id}:{position_id}`.
    /// See [`position_id_for_cached_point`].
    ///
    /// Note that the position is cached indefinitely. The caller is responsible for
    /// insuring that the point is valid before relying on the position.
    /// If the point is no longer valid, the cached position will be a foobar value.
    pub fn cache_buffer_point(
        &mut self,
        point: Point,
        position_id: impl Into<Cow<'static, str>>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.cached_buffer_points.insert(position_id.into(), point);

        // Force a re-render so that the position is cached via the editor element.
        ctx.notify();
    }

    fn handle_editor_settings_update(
        &mut self,
        _: ModelHandle<EditorSettings>,
        evt: &EditorSettingsChangedEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        // Ensure our cached copy of these settings are up-to-date.
        match evt {
            EditorSettingsChangedEvent::CursorDisplayState { .. } => {
                if self.cursor_display_override.is_some() {
                    self.cursor_display_override =
                        Some(*EditorSettings::as_ref(ctx).cursor_display_type);
                }
            }
            EditorSettingsChangedEvent::AutocompleteSymbols { .. } => {
                self.autocomplete_symbols_setting =
                    *EditorSettings::as_ref(ctx).autocomplete_symbols;
            }
            EditorSettingsChangedEvent::CursorBlinkEnabled { .. } => {}
        }
    }

    /// Returns the buffer point for the corresponding offset.
    pub fn point_for_offset(&self, offset: impl ToCharOffset, ctx: &AppContext) -> Result<Point> {
        let buffer = self.editor_model.as_ref(ctx).buffer(ctx);
        let char_offset = offset.to_char_offset(buffer)?;
        buffer.to_point(char_offset)
    }

    /// Set placeholder text that appears when buffer matches the given prefix.
    /// Use empty string prefix "" for the default placeholder (shown when buffer is empty).
    pub fn set_placeholder_text_with_prefix(
        &mut self,
        prefix: impl Into<String>,
        text: impl Into<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        Arc::make_mut(&mut self.placeholder_texts).insert(prefix.into(), text.into());
        ctx.notify();
    }

    /// Convenience method for setting the default placeholder (empty prefix).
    pub fn set_placeholder_text(&mut self, text: impl Into<String>, ctx: &mut ViewContext<Self>) {
        self.set_placeholder_text_with_prefix("", text, ctx);
    }

    pub fn set_base_buffer_text(&mut self, base_content: String, ctx: &mut ViewContext<Self>) {
        self.base_buffer_text = base_content;
        ctx.notify();
    }

    /// Remove a specific placeholder by prefix.
    pub fn clear_placeholder_text_with_prefix(
        &mut self,
        prefix: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        Arc::make_mut(&mut self.placeholder_texts).remove(prefix);
        ctx.notify();
    }

    /// Convenience method for clearing the default placeholder (empty prefix).
    pub fn clear_placeholder_text(&mut self, ctx: &mut ViewContext<Self>) {
        self.clear_placeholder_text_with_prefix("", ctx);
    }

    /// Clear all placeholder texts.
    pub fn clear_all_placeholder_text(&mut self) {
        self.placeholder_texts = Arc::new(HashMap::new());
    }

    pub fn is_single_line(&self) -> bool {
        self.single_line
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn set_autogrow(&mut self, autogrow: bool) {
        self.autogrow = autogrow;
    }

    /// Replaces the editor's enter-key settings at runtime (effective next keystroke).
    pub fn set_enter_settings(&mut self, settings: EnterSettings) {
        self.enter_settings = settings;
    }

    /// Returns the current enter-key settings (for tests asserting applied settings).
    pub fn enter_settings(&self) -> EnterSettings {
        self.enter_settings.clone()
    }

    pub fn set_propagate_vertical_navigation_keys(
        &mut self,
        propagate_vertical_navigation_keys: PropagateAndNoOpNavigationKeys,
    ) {
        self.propagate_vertical_navigation_keys = propagate_vertical_navigation_keys;
    }

    pub fn set_propagate_horizontal_navigation_keys(
        &mut self,
        propagate_horizontal_navigation_keys: PropagateHorizontalNavigationKeys,
    ) {
        self.propagate_horizontal_navigation_keys = propagate_horizontal_navigation_keys;
    }

    pub fn can_edit<C: ModelAsRef>(&self, ctx: &C) -> bool {
        self.model().as_ref(ctx).can_edit()
    }

    pub fn can_select<C: ModelAsRef>(&self, ctx: &C) -> bool {
        self.model().as_ref(ctx).can_select()
    }

    pub fn interaction_state<C: ModelAsRef>(&self, ctx: &C) -> InteractionState {
        self.editor_model.as_ref(ctx).interaction_state()
    }

    pub fn set_interaction_state(
        &mut self,
        interaction_state: InteractionState,
        ctx: &mut ViewContext<Self>,
    ) {
        self.editor_model.update(ctx, |model, _| {
            model.set_interaction_state(interaction_state);
        });
        ctx.notify();
    }

    pub fn set_autocomplete_symbols_allowed(&mut self, enable: bool) {
        self.autocomplete_symbols_allowed = enable;
    }

    /// This should not be exposed as a public function.
    /// If you (as a consumer) need access to the model, you
    /// should interface through the [`EditorView`]`.
    fn model(&self) -> &ModelHandle<EditorModel> {
        &self.editor_model
    }

    /// Returns whether or not the editor is empty.
    pub fn is_empty(&self, ctx: &AppContext) -> bool {
        self.editor_model.as_ref(ctx).buffer(ctx).is_empty()
    }

    // Returns the buffer characters count (similar to String::Chars::Count).
    pub fn buffer_size(&self, ctx: &AppContext) -> CharOffset {
        self.editor_model.as_ref(ctx).buffer(ctx).len()
    }

    pub fn buffer_text(&self, ctx: &AppContext) -> String {
        self.editor_model.as_ref(ctx).buffer_text(ctx)
    }

    /// Returns the buffer text before the last edit.
    /// If there weren't any edits, returns the base text.
    /// In the case of an undo / redo, the last buffer text would be the
    /// buffer text before the undo / redo.
    ///
    /// Example:
    /// 1. Suppose the buffer text starts as "a".
    /// 2. "b" is inserted.
    /// 3. Undo.
    ///
    /// The last buffer text is now "ab".
    pub fn last_buffer_text<'a>(&self, ctx: &'a AppContext) -> &'a str {
        self.editor_model.as_ref(ctx).last_buffer_text()
    }

    /// Gets the last action in the editor model's undo stack.
    pub fn get_last_action<C: ModelAsRef>(&self, ctx: &C) -> Option<PlainTextEditorViewAction> {
        self.editor_model.as_ref(ctx).last_action(ctx)
    }

    fn scroll(&mut self, scroll_position: Vector2F, ctx: &mut ViewContext<Self>) {
        *self.scroll_position.lock() = Vector2F::new(scroll_position.x(), scroll_position.y());
        ctx.notify();
    }

    fn select(&mut self, arg: &SelectAction, ctx: &mut ViewContext<Self>) {
        self.maybe_commit_incomplete_ime_text(ctx);
        if self.can_select(ctx) {
            match arg {
                SelectAction::Begin { position, add } => {
                    self.begin_selection(position.point, position.clamp_direction, *add, ctx)
                }
                SelectAction::Update {
                    position,
                    scroll_position,
                } => self.update_selection(*position, *scroll_position, ctx),
                SelectAction::Extend {
                    position,
                    scroll_position,
                } => self.extend_selection(*position, *scroll_position, ctx),
                SelectAction::End => self.end_selection(ctx),
            }
        }
    }

    fn begin_selection(
        &mut self,
        position: DisplayPoint,
        clamp_direction: ClampDirection,
        add: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.focused {
            ctx.focus_self();
            ctx.emit(Event::Activate);
            if self.select_all_on_focus {
                self.select_all(ctx);
                return;
            }
        }

        self.change_selections(ctx, |editor_model, ctx| {
            let cursor = display_point_to_anchor_clamped(editor_model, position, Bias::Left, ctx);

            let selection = LocalSelection {
                selection: Selection {
                    start: cursor.clone(),
                    end: cursor,
                    reversed: false,
                },
                clamp_direction,
                goal_start_column: None,
                goal_end_column: None,
            };

            let pending = Some(LocalPendingSelection {
                selection: selection.clone(),
                selection_mode: SelectionMode::Chars,
                starting_selection: selection.clone(),
                is_single_selection: !add,
            });

            let selections = if !add {
                vec1![selection]
            } else {
                editor_model.selections(ctx).clone()
            };

            editor_model.change_selections(
                LocalSelections {
                    pending,
                    selections,
                    marked_text_state: Default::default(),
                },
                ctx,
            );
        });
    }

    fn update_selection(
        &mut self,
        position: DisplayPoint,
        scroll_position: Vector2F,
        ctx: &mut ViewContext<Self>,
    ) {
        self.change_selections(ctx, |editor_model, ctx| {
            if let Some(mut pending_selection) = editor_model.pending_selection(ctx).cloned() {
                let cursor =
                    display_point_to_anchor_clamped(editor_model, position, Bias::Left, ctx);

                pending_selection
                    .selection
                    .set_head(editor_model.buffer(ctx), cursor.clone());

                let new_selections = if pending_selection.is_single_selection {
                    let buffer = editor_model.buffer(ctx);
                    let mut selection = editor_model.first_selection(ctx).clone();
                    let selection_mode = pending_selection.selection_mode;
                    let starting_selection = pending_selection.starting_selection.clone();

                    // Variables used to compare position of current cursor relative to starting_selection
                    let ordering = cursor
                        .cmp(starting_selection.head(), buffer)
                        .expect("Anchors should be comparable");

                    let is_cursor_in_range = starting_selection.range(buffer).contains(
                        &cursor
                            .to_point(buffer)
                            .expect("Should be able to get point from cursor"),
                    );

                    // Update current selection based on head and tails of cursor and starting_selection,
                    // extending to nearby words or lines based on selection mode
                    let mut head = starting_selection.head().clone();
                    let mut tail = starting_selection.tail().clone();

                    match selection_mode {
                        SelectionMode::Lines => {
                            if ordering == Ordering::Greater {
                                // Select next line(s)
                                match buffer.line_len(position.row()) {
                                    Ok(end_col) => {
                                        head = buffer
                                            .anchor_before(Point::new(position.row(), end_col))
                                            .expect("Anchor should exist")
                                    }
                                    Err(_) => report_error!(
                                        "Update selection is called with invalid position"
                                    ),
                                }
                            } else if is_cursor_in_range || ordering == Ordering::Equal {
                                selection = starting_selection.clone();
                            } else {
                                // Select previous line(s)
                                tail = starting_selection.head().clone();
                                head = buffer
                                    .anchor_before(Point::new(position.row(), 0))
                                    .expect("Anchor should exist");
                            }
                        }
                        SelectionMode::Words => {
                            if ordering == Ordering::Greater {
                                // Select next word(s)
                                head = editor_model.get_word_end(
                                    position,
                                    WordBoundariesPolicy::Default,
                                    ctx,
                                );
                            } else if is_cursor_in_range || ordering == Ordering::Equal {
                                selection = starting_selection.clone();
                            } else {
                                // Select previous word(s)
                                tail = starting_selection.head().clone();
                                head = editor_model.get_word_start(
                                    position,
                                    WordBoundariesPolicy::Default,
                                    ctx,
                                );
                            }
                        }
                        SelectionMode::Chars => {
                            head = cursor;
                        }
                    }

                    selection.set_head(buffer, head);
                    selection.set_tail(buffer, tail);

                    // Update the current selection to display the correct selection while dragging.
                    // And update the pending selection to display the correct selection after release.
                    LocalSelections {
                        selections: vec1![selection.clone()],
                        pending: Some(LocalPendingSelection {
                            selection,
                            selection_mode,
                            starting_selection,
                            is_single_selection: true,
                        }),
                        marked_text_state: Default::default(),
                    }
                } else {
                    LocalSelections {
                        pending: Some(pending_selection.clone()),
                        selections: editor_model.selections(ctx).clone(),
                        marked_text_state: Default::default(),
                    }
                };

                editor_model.change_selections(new_selections, ctx);
            } else {
                report_error!("update_selection dispatched with no pending selection");
            }
        });

        *self.scroll_position.lock() = scroll_position;
    }

    fn extend_selection(
        &mut self,
        position: DisplayPoint,
        scroll_position: Vector2F,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.focused {
            ctx.focus_self();
            ctx.emit(Event::Activate);
        }

        self.change_selections(ctx, |editor_model, ctx| {
            let cursor = display_point_to_anchor_clamped(editor_model, position, Bias::Left, ctx);

            let mut selection = editor_model.first_selection(ctx).clone();
            selection.set_head(editor_model.buffer(ctx), cursor);
            editor_model.change_selections(vec1![selection], ctx);
        });

        *self.scroll_position.lock() = scroll_position;
        ctx.notify();
    }

    fn end_selection(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            if let Some(pending_selection) = editor_model.pending_selection(ctx).cloned() {
                // Insert the starting selection.
                let new_selections = if pending_selection.is_single_selection {
                    vec1![pending_selection.starting_selection]
                } else {
                    let mut new_selections = editor_model.selections(ctx).clone();
                    let ix = editor_model.selection_insertion_index(
                        pending_selection.starting_selection.start(),
                        ctx,
                    );
                    new_selections.insert(ix, pending_selection.starting_selection);
                    new_selections
                };
                // This will also clear the pending selection.
                editor_model.change_selections(new_selections, ctx);

                // Insert the pending selection itself.
                let mut new_selections = editor_model.selections(ctx).clone();
                let ix = editor_model
                    .selection_insertion_index(pending_selection.selection.start(), ctx);
                new_selections.insert(ix, pending_selection.selection);
                editor_model.change_selections(new_selections, ctx);
            } else {
                report_error!("end_selection dispatched with no pending selection");
            }
        });
    }

    pub fn select_line(&mut self, position: &DisplayPoint, ctx: &mut ViewContext<Self>) {
        let row = position.row();

        self.change_selections(ctx, |editor_model, ctx| {
            let buffer = editor_model.buffer(ctx);
            match buffer.line_len(row) {
                Ok(end_col) => {
                    let selection = LocalSelection {
                        selection: Selection {
                            start: buffer
                                .anchor_after(Point::new(row, 0))
                                .expect("Anchor should exist"),
                            end: buffer
                                .anchor_before(Point::new(row, end_col))
                                .expect("Anchor should exist"),
                            reversed: false,
                        },
                        clamp_direction: Default::default(),
                        goal_start_column: None,
                        goal_end_column: None,
                    };

                    // Continue expecting selection
                    editor_model.change_selections(
                        LocalSelections {
                            pending: Some(LocalPendingSelection {
                                selection: selection.clone(),
                                selection_mode: SelectionMode::Lines,
                                starting_selection: selection.clone(),
                                is_single_selection: true,
                            }),
                            selections: vec1![selection],
                            marked_text_state: Default::default(),
                        },
                        ctx,
                    );
                }
                Err(_) => {
                    report_error!("select_line is called with invalid position");
                }
            }
        });
    }

    /// Selects the word at `position`; triggered by the initial double-click (not a drag).
    pub fn select_word(&mut self, position: &DisplayPoint, ctx: &mut ViewContext<Self>) {
        let position = *position;

        self.change_selections(ctx, |editor_model, ctx| {
            let start = editor_model.get_word_start(position, WordBoundariesPolicy::Default, ctx);
            let end = editor_model.get_word_end(position, WordBoundariesPolicy::Default, ctx);

            let selection = LocalSelection {
                selection: Selection {
                    start,
                    end,
                    reversed: false,
                },
                clamp_direction: Default::default(),
                goal_start_column: None,
                goal_end_column: None,
            };

            // Update selections to select word
            editor_model.change_selections(
                LocalSelections {
                    pending: Some(LocalPendingSelection {
                        selection: selection.clone(),
                        selection_mode: SelectionMode::Words,
                        starting_selection: selection.clone(),
                        is_single_selection: true,
                    }),
                    selections: vec1![selection],
                    marked_text_state: Default::default(),
                },
                ctx,
            );
        });
    }

    pub fn is_selecting(&self, ctx: &AppContext) -> bool {
        self.editor_model.as_ref(ctx).is_selecting(ctx)
    }

    pub fn copy(&mut self, ctx: &mut ViewContext<Self>) {
        if self.is_password {
            return;
        }

        if self.can_select(ctx) {
            let text = self.selected_text(ctx);
            if text.is_empty() {
                ctx.emit(Event::Copy);
            } else {
                ctx.clipboard().write(ClipboardContent::plain_text(text));
            }
        }
    }

    pub fn cut(&mut self, ctx: &mut ViewContext<Self>) {
        if self.is_password {
            return;
        }
        self.copy(ctx);
        self.backspace(ctx);
    }

    pub fn paste(&mut self, ctx: &mut ViewContext<Self>) {
        // If this editor does not delegate paste handling, insert clipboard text content.
        // When paste handling is delegated, the parent view is responsible for processing the
        // paste.
        if !self.delegate_paste_handling {
            // Read clipboard contents
            let content = ctx.clipboard().read();

            let clipboard_content_str = self.clipboard_text_content(content);
            self.user_initiated_insert(
                &clipboard_content_str,
                PlainTextEditorViewAction::Paste,
                ctx,
            );
        }

        ctx.emit(Event::Paste);
    }

    fn middle_click_paste(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.middle_click_paste {
            return;
        }

        let clipboard_content = self.middle_click_paste_content(ctx);
        if let Some(clipboard_content) = clipboard_content {
            self.user_initiated_insert(&clipboard_content, PlainTextEditorViewAction::Paste, ctx);
            ctx.emit(Event::MiddleClickPaste);
        }
        ctx.focus_self();
    }

    // Pastes the most recently cut text within the editor.
    fn yank(&mut self, ctx: &mut ViewContext<Self>) {
        let text = self.internal_clipboard.clone();
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::Yank,
                EditOrigin::UserInitiated,
                move |editor_model, ctx| {
                    editor_model.insert(text.as_str(), None, ctx);
                },
            ),
        );
    }

    /// Returns whether or not the editor's current buffer text is equal to
    /// the base buffer text it was supplied with.
    pub fn is_dirty(&self, ctx: &AppContext) -> bool {
        !self.buffer_text(ctx).eq(&self.base_buffer_text)
    }

    pub fn selected_text<C: ModelAsRef>(&self, ctx: &C) -> String {
        self.editor_model.as_ref(ctx).selected_text(ctx)
    }

    pub fn selected_text_strings(&self, ctx: &mut ViewContext<Self>) -> Vec<String> {
        self.editor_model.as_ref(ctx).selected_text_strings(ctx)
    }

    pub fn start_byte_index_of_first_selection<A: ModelAsRef>(&self, ctx: &A) -> ByteOffset {
        let model = self.model().as_ref(ctx);
        let buffer = model.buffer(ctx);
        model
            .first_selection(ctx)
            .head()
            .to_byte_offset(buffer)
            .expect("Selection must be convertible to byte offset")
    }

    /// Finds the start byte of the token under the given point (the offset at
    /// the start word boundary of the underlying token)
    pub fn start_byte_offset_at_point(
        &self,
        point: &DisplayPoint,
        ctx: &AppContext,
    ) -> Option<ByteOffset> {
        let model = self.model().as_ref(ctx);
        let buffer = model.buffer(ctx);
        let map = model.display_map(ctx);

        let position = point.to_buffer_point(map, Bias::Left, ctx).ok()?;
        let mut word_starts = buffer
            .word_starts_backward_from_offset_inclusive(position)
            .ok()?;
        word_starts
            .next()
            .and_then(|word_start| word_start.to_byte_offset(buffer).ok())
    }

    /// Returns the starting byte index position of the last selection.
    pub fn start_byte_index_of_last_selection<A: ModelAsRef>(&self, ctx: &A) -> ByteOffset {
        let model = self.model().as_ref(ctx);
        let buffer = model.buffer(ctx);
        model.last_selection(ctx).to_byte_offset(buffer).sorted().0
    }

    /// Returns the ending byte index position of the last selection.
    pub fn end_byte_index_of_last_selection<A: ModelAsRef>(&self, ctx: &A) -> ByteOffset {
        let model = self.model().as_ref(ctx);
        let buffer = model.buffer(ctx);
        model.last_selection(ctx).to_byte_offset(buffer).sorted().1
    }

    // Clears editor buffer but does not clear the undo/redo stack.
    pub fn clear_buffer(&mut self, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::ClearBuffer,
                EditOrigin::UserInitiated,
                |editor_model, ctx| {
                    editor_model.clear_buffer(ctx);
                },
            ),
        );
    }

    // Clears editor buffer and conditionally resets the undo stack.
    pub fn system_clear_buffer(&mut self, reset_undo_stack: bool, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::ClearBuffer,
                EditOrigin::SystemEdit,
                |editor_model, ctx| {
                    editor_model.clear_buffer(ctx);
                },
            ),
        );

        if reset_undo_stack {
            // Reset the undo stack _after_ clearing the buffer so that the change
            // is not pushed onto the stack.
            self.editor_model.update(ctx, |model, ctx| {
                model.reset_undo_redo_stack(ctx);
            });
        }
    }

    // Clears editor buffer but also resets the undo/redo stack.
    pub fn clear_buffer_and_reset_undo_stack(&mut self, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::ClearBuffer,
                EditOrigin::UserInitiated,
                |editor_model, ctx| {
                    editor_model.clear_buffer(ctx);
                },
            ),
        );

        // Reset the undo stack _after_ clearing the buffer so that the change
        // is not pushed onto the stack.
        self.editor_model.update(ctx, |model, ctx| {
            model.reset_undo_redo_stack(ctx);
        });
    }

    // Replaces the current editor buffer with temporary contents (e.g. commands from history up).
    // This will not create new undo items in the undo/redo stack.
    pub fn set_buffer_text_ignoring_undo(&mut self, content: &str, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer_options(
                PlainTextEditorViewAction::ReplaceBuffer,
                EditOrigin::UserInitiated,
                UpdateBufferOption::IsEphemeral,
                |model, ctx| {
                    model.clear_buffer(ctx);
                    model.insert(content, None, ctx);
                },
            ),
        );
    }

    // Inserts selected text into buffer, without creating new undo items.
    pub fn insert_selected_text_to_buffer_ignoring_undo(
        &mut self,
        content: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer_options(
                PlainTextEditorViewAction::InsertSelectedText,
                EditOrigin::UserInitiated,
                UpdateBufferOption::IsEphemeral,
                |model, ctx| {
                    model.insert_selected_text(content, ctx);
                },
            ),
        );
    }

    /// As a system edit, reset the buffer to the given text and clear the undo/redo stack. This is
    /// useful when new buffer contents are synced from another source.
    ///
    /// Also see [`Self::set_buffer_text`], which makes similar updates, but as a user-initiated
    /// edit visible in the undo stack.
    pub fn system_reset_buffer_text(&mut self, content: &str, ctx: &mut ViewContext<Self>) {
        let interaction_state = self.interaction_state(ctx);
        self.set_interaction_state(InteractionState::Editable, ctx);

        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::ReplaceBuffer,
                EditOrigin::SystemEdit,
                |editor_model, ctx| {
                    editor_model.clear_buffer(ctx);
                    editor_model.insert(content, None, ctx);
                },
            ),
        );

        // Reset the undo stack _after_ replacing the buffer so that the change
        // is not pushed onto the stack.
        self.editor_model.update(ctx, |model, ctx| {
            model.reset_undo_redo_stack(ctx);
        });

        self.set_interaction_state(interaction_state, ctx);
        ctx.notify();
    }

    /// Replaces the current editor buffer. This will create a new undo item in undo/redo stack.
    /// This function works regardless of the interaction state (i.e. it'll allow
    /// the contents to change even if the interaction state is disabled or selectable).
    pub fn set_buffer_text(&mut self, content: &str, ctx: &mut ViewContext<Self>) {
        let interaction_state = self.interaction_state(ctx);
        self.set_interaction_state(InteractionState::Editable, ctx);

        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::ReplaceBuffer,
                EditOrigin::UserInitiated,
                |editor_model, ctx| {
                    editor_model.clear_buffer(ctx);
                    editor_model.insert(content, None, ctx);
                },
            ),
        );

        self.set_interaction_state(interaction_state, ctx);
        ctx.notify();
    }

    pub fn set_buffer_text_with_base_buffer(&mut self, content: &str, ctx: &mut ViewContext<Self>) {
        self.set_base_buffer_text(content.to_string(), ctx);
        self.set_buffer_text(content, ctx);
    }

    pub fn replace_first_n_characters(
        &mut self,
        n: CharOffset,
        text: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::ReplaceBuffer,
                EditOrigin::UserInitiated,
                |editor_model, ctx| {
                    editor_model.replace_first_n_characters(n, text, ctx);
                },
            ),
        );
    }

    pub fn replace_last_n_characters(
        &mut self,
        n: CharOffset,
        text: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::ReplaceBuffer,
                EditOrigin::UserInitiated,
                |editor_model, ctx| {
                    editor_model.replace_last_n_characters(n, text, ctx);
                },
            ),
        );
    }

    pub fn cut_word_left(&mut self, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new()
                .with_update_buffer(
                    PlainTextEditorViewAction::CutWordLeft,
                    EditOrigin::UserInitiated,
                    |editor_model, ctx| editor_model.shell_cut(ctx),
                )
                .with_change_selections(|editor_model, ctx| {
                    editor_model.select_word_left(ctx);
                }),
        );
    }

    pub fn delete_word_left(&mut self, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new()
                .with_update_buffer(
                    PlainTextEditorViewAction::DeleteWordLeft,
                    EditOrigin::UserInitiated,
                    |editor_model, ctx| editor_model.insert("", None, ctx),
                )
                .with_change_selections(|editor_model, ctx| editor_model.select_word_left(ctx)),
        );
    }

    pub fn delete_word_right(&mut self, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new()
                .with_update_buffer(
                    PlainTextEditorViewAction::DeleteWordRight,
                    EditOrigin::UserInitiated,
                    |editor_model, ctx| editor_model.insert("", None, ctx),
                )
                .with_change_selections(|editor_model, ctx| editor_model.select_word_right(ctx)),
        );
    }

    pub fn cut_word_right(&mut self, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new()
                .with_update_buffer(
                    PlainTextEditorViewAction::CutWordRight,
                    EditOrigin::UserInitiated,
                    |editor_model, ctx| editor_model.shell_cut(ctx),
                )
                .with_change_selections(|editor_model, ctx| {
                    editor_model.select_word_right(ctx);
                }),
        );
    }

    /// Clear any selections, leaving the cursor at the end of the first selection.
    pub fn clear_selections(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model.clear_selections(ctx);
        });
    }

    fn tab(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            match self.propagate_vertical_navigation_keys {
                PropagateAndNoOpNavigationKeys::Always => {
                    ctx.emit(Event::Navigate(NavigationKey::Tab))
                }
                _ => self.handle_tab(ctx),
            };
        }
    }

    pub fn handle_tab(&mut self, ctx: &mut ViewContext<Self>) {
        let buffer = self.editor_model.as_ref(ctx).buffer(ctx);
        let selections = self.editor_model.as_ref(ctx).selections(ctx);
        match selections.len() {
            1 => {
                let selection = selections.first();
                // Indent selection if there is a selection
                // Indent is analogous to indenting in a code editor, where we
                // want to move the selected line(s) forward, and we don't
                // want to move the cursor.
                if !selection.is_cursor_only(buffer) {
                    self.indent(ctx);
                } else {
                    self.edit(
                        ctx,
                        Edits::new().with_update_buffer(
                            PlainTextEditorViewAction::Tab,
                            EditOrigin::UserInitiated,
                            |editor_model, ctx| editor_model.insert("    ", None, ctx),
                        ),
                    );
                }
            }
            _ => self.edit(
                ctx,
                Edits::new().with_update_buffer(
                    PlainTextEditorViewAction::Tab,
                    EditOrigin::UserInitiated,
                    |editor_model, ctx| editor_model.insert("    ", None, ctx),
                ),
            ),
        }
    }

    fn indent(&mut self, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::Indent,
                EditOrigin::UserInitiated,
                |editor_model, ctx| {
                    editor_model.indent(ctx);
                },
            ),
        );
    }

    fn shift_tab(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            match self.propagate_vertical_navigation_keys {
                PropagateAndNoOpNavigationKeys::Always => {
                    ctx.emit(Event::Navigate(NavigationKey::ShiftTab))
                }
                _ => self.unindent(ctx),
            };
        }
    }

    pub fn unindent(&mut self, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::Indent,
                EditOrigin::UserInitiated,
                |editor_model, ctx| {
                    editor_model.unindent(ctx);
                },
            ),
        );
    }

    pub fn insert_selected_text(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::InsertSelectedText,
                EditOrigin::UserInitiated,
                |editor_model, ctx| {
                    editor_model.insert_selected_text(text, ctx);
                },
            ),
        );
    }

    /// Selects the given ranges and replaces the text at the selection with the given `text`.
    pub fn select_and_replace(
        &mut self,
        text: &str,
        selection_ranges: impl IntoIterator<Item = Range<ByteOffset>>,
        action: PlainTextEditorViewAction,
        ctx: &mut ViewContext<Self>,
    ) {
        self.edit(
            ctx,
            Edits::new()
                .with_update_buffer(action, EditOrigin::UserInitiated, |editor_model, ctx| {
                    editor_model.insert(text, None /* text_style */, ctx);
                })
                .with_change_selections(|editor_model, ctx| {
                    editor_model
                        .select_ranges_by_offset(selection_ranges, ctx)
                        .expect("byte index selection should be insertable")
                }),
        );
    }

    pub fn user_initiated_insert(
        &mut self,
        text: &str,
        action: PlainTextEditorViewAction,
        ctx: &mut ViewContext<Self>,
    ) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                action,
                EditOrigin::UserInitiated,
                |editor_model, ctx| {
                    editor_model.insert(text, None, ctx);
                },
            ),
        );
    }

    pub fn system_insert(
        &mut self,
        text: &str,
        action: PlainTextEditorViewAction,
        ctx: &mut ViewContext<Self>,
    ) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(action, EditOrigin::SystemEdit, |editor_model, ctx| {
                editor_model.insert(text, None, ctx);
            }),
        );
    }

    /// Performs a delete operation as a system edit (not user-initiated)
    /// Deletes text in the specified range
    pub fn system_delete(&mut self, range: Range<ByteOffset>, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::Delete,
                EditOrigin::SystemEdit,
                |editor_model, ctx| {
                    // Convert ByteOffset to CharOffset properly to handle multi-byte characters
                    let buffer = editor_model.buffer(ctx);
                    match (
                        range.start.to_char_offset(buffer),
                        range.end.to_char_offset(buffer),
                    ) {
                        (Ok(start_char), Ok(end_char)) => {
                            let char_range = start_char..end_char;
                            if let Err(error) = editor_model.buffer_edit([char_range], "", ctx) {
                                report_error!(error.context("error performing system delete"));
                            }
                        }
                        (Err(error), _) | (_, Err(error)) => {
                            report_error!(error.context(
                                "error converting byte offset to char offset for system delete"
                            ));
                        }
                    }
                },
            ),
        );
    }

    /// Selects ranges by `ByteOffset`. Note if the selections specified in `ranges` are empty, the
    /// selections are not set since there must be at least one selection.
    pub fn select_ranges_by_byte_offset(
        &mut self,
        selection_ranges: impl IntoIterator<Item = Range<ByteOffset>>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model
                .select_ranges_by_offset(selection_ranges, ctx)
                .expect("byte index selection should be insertable");
        });
    }

    pub fn text_style_runs<'a>(
        &'a self,
        ctx: &'a AppContext,
    ) -> impl 'a + Iterator<Item = TextRun> {
        let model = self.editor_model.as_ref(ctx);
        model.buffer(ctx).text_style_runs()
    }

    pub fn clear_text_style_runs(&mut self, ctx: &mut ViewContext<Self>) {
        self.editor_model.update(ctx, |model, ctx| {
            model.clear_all_styles(ctx);
        });
    }

    pub fn update_buffer_styles<I, S>(
        &mut self,
        old_ranges: I,
        text_style_operation: TextStyleOperation,
        ctx: &mut ViewContext<Self>,
    ) where
        I: IntoIterator<Item = Range<S>>,
        S: ToCharOffset,
    {
        self.editor_model.update(ctx, |model, ctx| {
            model.update_buffer_styles(old_ranges, text_style_operation, ctx)
        });
    }

    /// Inserts styles into the editor, rendering the ranges marked in `sorted_styles` with the
    /// necessary style.
    ///
    /// Note it up to the caller to ensure `sorted_styles` is sorted in ascending order based on the
    /// start of the range and that each range is disjoint. If a range is not disjoint, we will log a warning and skip it.
    pub fn insert_with_styles(
        &mut self,
        text: &str,
        sorted_styles: &[(Range<ByteOffset>, TextStyle)],
        action: PlainTextEditorViewAction,
        ctx: &mut ViewContext<Self>,
    ) {
        let mut final_styles = vec![];
        let mut last_range_end = ByteOffset::from(0);

        for (range, style) in sorted_styles {
            match text.get(last_range_end.as_usize()..range.start.as_usize()) {
                Some(last_range_end_to_curr_range_start) => {
                    final_styles.push((last_range_end_to_curr_range_start, TextStyle::new()));
                }
                None => log::warn!("last_range_end_to_curr_range_start is not a valid range"),
            }

            match text.get(range.start.as_usize()..range.end.as_usize()) {
                Some(curr_range_start_to_end) => {
                    final_styles.push((curr_range_start_to_end, *style));
                }
                None => log::warn!("curr_range_start_to_end is not a valid range"),
            }

            last_range_end = range.end;
        }

        match text.get(last_range_end.as_usize()..text.len()) {
            Some(remaining_text) => {
                final_styles.push((remaining_text, TextStyle::new()));
            }
            None => log::warn!("remaining_text is not a valid range"),
        }

        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                action,
                EditOrigin::UserInitiated,
                move |editor_model, ctx| {
                    for (text, style) in final_styles {
                        editor_model.insert(text, Some(style), ctx);
                    }
                },
            ),
        );
    }

    pub fn user_insert(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        let should_autocomplete_symbols =
            self.autocomplete_symbols_allowed && self.autocomplete_symbols_setting;
        let action = PlainTextEditorViewAction::from_inserted_str(text);
        self.edit(
            ctx,
            Edits::new().with_update_buffer(action, EditOrigin::UserTyped, |editor_model, ctx| {
                if should_autocomplete_symbols {
                    editor_model.insert_and_maybe_autocomplete_symbols(text, ctx);
                } else {
                    editor_model.insert_internal(text, None, SelectionInsertion::No, ctx);
                }
            }),
        );
    }

    fn ime_commit(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::UpdateMarkedText,
                EditOrigin::UserTyped,
                |editor_model, ctx| editor_model.clear_marked_text_and_commit(text, ctx),
            ),
        )
    }

    fn enter(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            match self.enter_settings.enter {
                EnterAction::InsertNewLineIfMultiLine if !self.single_line => {
                    self.newline_internal(ctx)
                }
                _ => ctx.emit(Event::Enter),
            }
        }
    }

    fn shift_enter(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            match self.enter_settings.shift_enter {
                EnterAction::InsertNewLineIfMultiLine if !self.single_line => {
                    self.newline_internal(ctx)
                }
                _ => ctx.emit(Event::ShiftEnter),
            }
        }
    }

    fn alt_enter(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            match self.enter_settings.alt_enter {
                EnterAction::InsertNewLineIfMultiLine if !self.single_line => {
                    self.newline_internal(ctx)
                }
                _ => ctx.emit(Event::AltEnter),
            }
        }
    }

    fn newline_internal(&mut self, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::NewLine,
                EditOrigin::UserInitiated,
                |editor_model, ctx| editor_model.insert("\n", None, ctx),
            ),
        );
    }

    fn newline(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            if self.single_line {
                // For single-line editors, inserting a newline results in us propagating the
                // shift-enter event to the parent view.
                ctx.emit(Event::ShiftEnter);
            } else {
                self.newline_internal(ctx);
            }
        }
    }

    fn ctrl_enter(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            match self.enter_settings.ctrl_enter {
                EnterAction::InsertNewLineIfMultiLine if !self.single_line => {
                    self.newline_internal(ctx)
                }
                _ => (),
            }
            ctx.emit(Event::CtrlEnter)
        }
    }

    pub fn backspace(&mut self, ctx: &mut ViewContext<Self>) {
        if self.buffer_text(ctx).is_empty() {
            ctx.emit(Event::BackspaceOnEmptyBuffer);
        } else if self.single_cursor_at_buffer_start(ctx) {
            ctx.emit(Event::BackspaceAtBeginningOfBuffer);
        }

        // Only select left for empty selections
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::Backspace,
                EditOrigin::UserInitiated,
                |editor_model, ctx| {
                    if editor_model.consecutive_autocomplete_insertion_edits_counter() > 0 {
                        editor_model.remove_before_and_after_cursor(ctx);
                    } else {
                        editor_model.backspace(ctx);
                    }
                },
            ),
        );
    }

    /// For all cursors, delete the character to the right of the cursor. For all selections, delete
    /// the characters in the selection.
    pub fn delete(&mut self, ctx: &mut ViewContext<Self>) {
        // Do not perform this edit if it would do nothing (to avoid adding to the UndoStack).
        if self.single_cursor_at_buffer_end(ctx) {
            return;
        }
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::Delete,
                EditOrigin::UserInitiated,
                |editor_model, ctx| {
                    if editor_model.consecutive_autocomplete_insertion_edits_counter() > 0 {
                        editor_model.remove_before_and_after_cursor(ctx);
                    } else {
                        editor_model.delete(ctx);
                    }
                },
            ),
        );
    }

    pub fn move_left(&mut self, stop_at_line_start: bool, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let map = editor_model.display_map(ctx);
            let mut new_selections = editor_model.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                let start = selection.start().to_display_point(map, ctx).unwrap();
                let end = selection.end().to_display_point(map, ctx).unwrap();

                if start != end {
                    selection.set_end(selection.start().clone());
                } else {
                    let cursor = map
                        .anchor_before(
                            movement::left(map, start, ctx, stop_at_line_start)
                                .expect("moving left should return a valid DisplayPoint"),
                            Bias::Left,
                            ctx,
                        )
                        .expect("DisplayPoint should convert to an Anchor");
                    selection.set_start(cursor.clone());
                    selection.set_end(cursor);
                }
                selection.set_reversed(false);
                selection.goal_start_column = None;
                selection.goal_end_column = None;
                // When we are moving left, we should keep the cursor on the same line
                // in favor of the line above.
                selection.clamp_direction = ClampDirection::Down;
            }
            editor_model.change_selections(new_selections, ctx);
        });
    }

    pub fn move_to_line_start(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model.cursor_line_start(false, ctx);
        });
    }

    pub fn move_to_paragraph_start(&mut self, ctx: &mut ViewContext<Self>) {
        // warp doesn't wrap the text, so basically each line is a paragraph.
        // this moves to the start of the paragraph (and the previous one if used multiple times).
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model.move_cursor(
                false,
                |buffer, selection| {
                    let start = selection.head().to_point(buffer).unwrap();
                    if start.row > 0 && start.column == 0 {
                        return Point::new(start.row - 1, 0);
                    }
                    Point::new(start.row, 0)
                },
                ctx,
            );
        });
    }
    pub fn move_to_buffer_start(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model.move_cursor(false, |_, _| Point::new(0, 0), ctx);
        });
    }

    pub fn select_to_line_start(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model.cursor_line_start(true, ctx);
        });
    }

    /// Moves each cursor to the start of the current visual (soft-wrapped) row,
    /// toggling between the row's first non-whitespace character and the very
    /// start of the row (classic "smart home" behavior, applied per visual row).
    ///
    /// When the soft-wrap layout has not been laid out yet, this falls back to
    /// the logical-line start (column 0 / first non-whitespace toggle).
    pub fn move_to_visual_line_start(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let map = editor_model.display_map(ctx);
            let buffer = editor_model.buffer(ctx);

            let mut new_selections = editor_model.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                let start = selection.start().to_display_point(map, ctx).unwrap();
                // Resolve the current visual (soft-wrapped) row's start column,
                // falling back to the logical-line start (column 0) when the
                // soft-wrap layout hasn't been laid out yet. The same "smart
                // home" toggle (row start <-> first non-whitespace) then runs in
                // both cases; when bounds are known we only scan whitespace
                // within the current visual row.
                let bounds = map.soft_wrapped_row_bounds(start, selection.clamp_direction);
                let row_start = bounds.as_ref().map_or(0, |bounds| bounds.start);
                let whitespace_scan_limit = bounds
                    .as_ref()
                    .map(|bounds| (bounds.end - bounds.start) as usize);
                let chars = buffer
                    .chars_at(
                        DisplayPoint::new(start.row(), row_start)
                            .to_buffer_point(map, Bias::Left, ctx)
                            .unwrap(),
                    )
                    .unwrap();
                let leading_whitespace = match whitespace_scan_limit {
                    Some(limit) => {
                        chars.take(limit).take_while(|c| c.is_whitespace()).count() as u32
                    }
                    None => chars.take_while(|c| c.is_whitespace()).count() as u32,
                };
                let first_non_whitespace = row_start + leading_whitespace;
                let cursor_start = if first_non_whitespace == start.column() {
                    row_start
                } else {
                    first_non_whitespace
                };
                let cursor = map
                    .anchor_before(
                        DisplayPoint::new(start.row(), cursor_start),
                        Bias::Left,
                        ctx,
                    )
                    .unwrap();
                selection.set_selection(Selection {
                    start: cursor.clone(),
                    end: cursor,
                    reversed: false,
                });
                selection.goal_start_column = None;
                selection.goal_end_column = None;
                // The caret now sits at the start of a visual row; clamp downward
                // so subsequent vertical movement treats it as the row's start.
                selection.clamp_direction = ClampDirection::Down;
            }
            editor_model.change_selections(new_selections, ctx);
        });
    }

    pub fn cmd_up(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            let point = Point::new(0, 0);
            let buffer = self.editor_model.as_ref(ctx).buffer(ctx);
            if self.single_cursor_on_first_row(ctx)
                && self.first_selection(ctx).start().to_point(buffer).unwrap() == point
            {
                ctx.emit(Event::CmdUpOnFirstRow);
            } else {
                self.cursor_top(ctx);
            }
        }
    }

    pub fn cursor_top(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let point = Point::new(0, 0);
            editor_model.reset_selections_to_point(&point, ctx);
        });
    }

    pub fn cursor_bottom(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let buffer = editor_model.buffer(ctx);
            let row = buffer.max_point().row;
            let col = buffer.line_len(row).unwrap();
            let point = Point::new(row, col);
            editor_model.reset_selections_to_point(&point, ctx);
        });
    }

    pub fn reset_selections_to_point(&mut self, point: &Point, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model.reset_selections_to_point(point, ctx);
        });
    }

    pub fn move_to_line_end(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model.cursor_line_end(false, ctx);
        });
    }

    pub fn move_to_buffer_end(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model.move_to_buffer_end(false /* keep_selection */, ctx);
        });
    }

    pub fn move_to_paragraph_end(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model.move_cursor(
                false,
                |buffer, selection| {
                    let end = selection.end().to_point(buffer).unwrap();
                    let paragraph_end = buffer.line_len(end.row).unwrap();
                    let max_point = buffer.max_point();
                    // we're at the end of paragraph, but not at the end of the text
                    if end.column == paragraph_end && end.row < max_point.row {
                        return Point::new(end.row + 1, buffer.line_len(end.row + 1).unwrap());
                    }
                    Point::new(end.row, buffer.line_len(end.row).unwrap())
                },
                ctx,
            );
        });
    }

    pub fn select_to_line_end(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model.cursor_line_end(true, ctx);
        });
    }

    pub fn move_and_select_to_buffer_start(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model.move_cursor(true, |_, _| Point::new(0, 0), ctx);
        });
    }

    pub fn move_and_select_to_buffer_end(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model.move_cursor(true, |buffer, _| buffer.max_point(), ctx);
        });
    }

    /// Moves each cursor to the end of the current visual (soft-wrapped) row.
    ///
    /// When the soft-wrap layout has not been laid out yet, this falls back to
    /// the logical-line end.
    pub fn move_to_visual_line_end(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let map = editor_model.display_map(ctx);
            let mut new_selections = editor_model.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                let end = selection
                    .end()
                    .to_display_point(map, ctx)
                    .expect("Should be able to get end point of selection.");
                // Use the end of the current visual (soft-wrapped) row,
                // falling back to the logical line end when the soft-wrap
                // layout isn't available.
                let cursor_column = map
                    .soft_wrapped_row_bounds(end, selection.clamp_direction)
                    .map(|bounds| bounds.end)
                    .unwrap_or_else(|| {
                        map.line_len(end.row(), ctx)
                            .expect("Should be able to get length of line at selection end.")
                    });
                let cursor = map
                    .anchor_before(
                        DisplayPoint::new(end.row(), cursor_column),
                        Bias::Right,
                        ctx,
                    )
                    .unwrap();
                selection.set_selection(Selection::single_cursor(cursor));
                selection.goal_start_column = None;
                selection.goal_end_column = None;
                // The caret now sits at the end of a visual row; clamp upward
                // so subsequent vertical movement treats it as the row's end.
                selection.clamp_direction = ClampDirection::Up;
            }
            editor_model.change_selections(new_selections, ctx);
        });
    }

    pub fn select_left(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let buffer = editor_model.buffer(ctx);
            let map = editor_model.display_map(ctx);
            let mut new_selections = editor_model.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                let head = selection.head().to_display_point(map, ctx).unwrap();
                let cursor = map
                    .anchor_before(
                        movement::left(map, head, ctx, /* stop at line start */ false).unwrap(),
                        Bias::Left,
                        ctx,
                    )
                    .unwrap();
                selection.set_head(buffer, cursor);
                selection.goal_start_column = None;
                selection.goal_end_column = None;
            }
            editor_model.change_selections(new_selections, ctx);
        });
    }

    pub fn move_right(&mut self, stop_at_line_end: bool, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let map = editor_model.display_map(ctx);
            let mut new_selections = editor_model.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                let start = selection.start().to_display_point(map, ctx).unwrap();
                let end = selection.end().to_display_point(map, ctx).unwrap();

                if start != end {
                    selection.set_start(selection.end().clone());
                } else {
                    let cursor = map
                        .anchor_before(
                            movement::right(map, end, ctx, stop_at_line_end)
                                .expect("moving right should return a valid DisplayPoint"),
                            Bias::Right,
                            ctx,
                        )
                        .expect("DisplayPoint should convert to an Anchor");
                    selection.set_start(cursor.clone());
                    selection.set_end(cursor);
                }
                selection.set_reversed(false);
                selection.goal_start_column = None;
                selection.goal_end_column = None;
                // When moving right, we should keep the cursor on the
                // same line in favor of the line below.
                selection.clamp_direction = ClampDirection::Up;
            }
            editor_model.change_selections(new_selections, ctx);
        });
    }

    pub fn escape(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_select(ctx) {
            if self.editor_model.as_ref(ctx).is_single_cursor_only(ctx) {
                ctx.emit(Event::Escape);
            } else {
                self.change_selections(ctx, |editor_model, ctx| {
                    editor_model.clear_selections(ctx);
                });
            }
        }
    }

    fn delete_all(&mut self, direction: CutDirection, cut: bool, ctx: &mut ViewContext<Self>) {
        let buffer = self.editor_model.as_ref(ctx).buffer(ctx);
        let map = self.editor_model.as_ref(ctx).display_map(ctx);

        let ranges = self
            .editor_model
            .as_ref(ctx)
            .selections(ctx)
            .iter()
            .map(|selection| {
                let start = selection.start().to_display_point(map, ctx).unwrap();
                let end = selection.end().to_display_point(map, ctx).unwrap();
                match direction {
                    CutDirection::Right => {
                        start..DisplayPoint::new(end.row(), buffer.line_len(end.row()).unwrap())
                    }
                    CutDirection::Left => {
                        let mut start = DisplayPoint::new(start.row(), 0);
                        if start == end {
                            // if the line was empty, move to the previous one
                            let head = selection.head().to_char_offset(buffer).unwrap();
                            start = head
                                .saturating_sub(&1.into())
                                .to_point(buffer)
                                .expect("Head offset should exist")
                                .to_display_point(map, ctx)
                                .expect("points should exist");
                        };
                        start..end
                    }
                }
            })
            .collect::<Vec<_>>();

        if cut {
            self.edit(
                ctx,
                Edits::new()
                    .with_update_buffer(
                        PlainTextEditorViewAction::CutAll,
                        EditOrigin::UserInitiated,
                        |editor_model, ctx| {
                            editor_model.shell_cut(ctx);
                        },
                    )
                    .with_change_selections(move |editor_model, ctx| {
                        editor_model
                            .select_ranges_by_display_point(ranges, ctx)
                            .expect("points should exist");
                    }),
            );
        } else {
            let ranges = ranges
                .iter()
                .map(|range| {
                    range
                        .start
                        .to_char_offset(map, Bias::Left, buffer, ctx)
                        .unwrap()
                        .as_usize()
                        ..range
                            .end
                            .to_char_offset(map, Bias::Left, buffer, ctx)
                            .unwrap()
                            .as_usize()
                })
                .collect::<Vec<_>>();

            self.edit(
                ctx,
                Edits::new().with_update_buffer(
                    PlainTextEditorViewAction::DeleteAll,
                    EditOrigin::UserInitiated,
                    move |editor_model, ctx| {
                        if let Err(error) = editor_model.buffer_edit(
                            merge_ranges(ranges).into_iter().map(|range| {
                                CharOffset::from(range.start)..CharOffset::from(range.end)
                            }),
                            "",
                            ctx,
                        ) {
                            report_error!(
                                error.context("error deleting all"),
                                extra: { "direction" => ?direction }
                            );
                        };
                    },
                ),
            );
        }
    }

    pub fn clear_and_copy_lines(&mut self, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new()
                .with_update_buffer(
                    PlainTextEditorViewAction::ClearAndCopyLines,
                    EditOrigin::UserInitiated,
                    |editor_model, ctx| {
                        editor_model.shell_cut(ctx);
                    },
                )
                .with_change_selections(|editor_model, ctx| {
                    let map = editor_model.display_map(ctx);
                    let buffer = editor_model.buffer(ctx);

                    let ranges = editor_model
                        .selections(ctx)
                        .iter()
                        .map(|selection| {
                            let position = selection.start().to_display_point(map, ctx).unwrap();
                            let row = position.row();
                            DisplayPoint::new(row, 0)
                                ..DisplayPoint::new(row, buffer.line_len(row).unwrap())
                        })
                        .collect::<Vec<_>>();

                    editor_model
                        .select_ranges_by_display_point(ranges, ctx)
                        .expect("range should exist");
                }),
        );
    }

    pub fn clear_lines(&mut self, ctx: &mut ViewContext<Self>) {
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::ClearLines,
                EditOrigin::UserInitiated,
                |editor_model, ctx| {
                    let buffer = editor_model.buffer(ctx);
                    let row_ranges = merge_ranges(
                        editor_model
                            .selections(ctx)
                            .iter()
                            .map(|selection| {
                                let start = selection.start().to_point(buffer).unwrap().row;
                                let end = selection.end().to_point(buffer).unwrap().row;
                                (start as usize)..(end as usize + 1)
                            })
                            .collect::<Vec<_>>(),
                    );

                    if let Err(error) = editor_model.buffer_edit(
                        row_ranges
                            .iter()
                            .map(|row_range| {
                                Point::new(row_range.start as u32, 0)
                                    .to_char_offset(buffer)
                                    .unwrap()
                                    ..Point::new(
                                        row_range.end as u32 - 1,
                                        buffer.line_len(row_range.end as u32 - 1).unwrap(),
                                    )
                                    .to_char_offset(buffer)
                                    .unwrap()
                            })
                            .collect::<Vec<_>>(),
                        "",
                        ctx,
                    ) {
                        report_error!(error.context("error clearing lines"));
                    }
                },
            ),
        );
    }

    fn cursor_forward_one_word(&mut self, select: bool, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let buffer = editor_model.buffer(ctx);
            let map = editor_model.display_map(ctx);

            let mut new_selections = editor_model.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                let end_position = if select {
                    selection.head().to_point(buffer).unwrap()
                } else {
                    selection.end().to_point(buffer).unwrap()
                };
                if let Ok(mut boundaries) = buffer.word_ends_from_offset_exclusive(end_position) {
                    let word_start = boundaries.next().unwrap_or(end_position);
                    let cursor = map
                        .anchor_before(
                            word_start.to_display_point(map, ctx).unwrap(),
                            Bias::Right,
                            ctx,
                        )
                        .unwrap();
                    if select {
                        selection.set_head(buffer, cursor);
                    } else {
                        selection.set_selection(Selection::single_cursor(cursor));
                    }
                    selection.goal_start_column = None;
                    selection.goal_end_column = None;
                }
            }
            editor_model.change_selections(new_selections, ctx);
        });
    }

    fn cursor_backward_one_word(&mut self, select: bool, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let buffer = editor_model.buffer(ctx);
            let map = editor_model.display_map(ctx);
            let mut new_selections = editor_model.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                let start_position = if select {
                    selection.head().to_point(buffer).unwrap()
                } else {
                    selection.start().to_point(buffer).unwrap()
                };
                if let Ok(mut word_starts) =
                    buffer.word_starts_backward_from_offset_exclusive(start_position)
                {
                    let word_start = word_starts.next().unwrap_or(start_position);
                    let cursor = map
                        .anchor_before(
                            word_start.to_display_point(map, ctx).unwrap(),
                            Bias::Left,
                            ctx,
                        )
                        .unwrap_or(Anchor::Start);
                    if select {
                        selection.set_head(buffer, cursor);
                    } else {
                        selection.set_selection(Selection::single_cursor(cursor));
                    }
                    selection.goal_start_column = None;
                    selection.goal_end_column = None;
                }
            }
            editor_model.change_selections(new_selections, ctx);
        });
    }

    fn cursor_forward_one_subword(&mut self, select: bool, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let buffer = editor_model.buffer(ctx);
            let map = editor_model.display_map(ctx);

            let mut new_selections = editor_model.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                let end_position = if select {
                    selection
                        .head()
                        .to_point(buffer)
                        .expect("Selection head must be convertible to a Point")
                } else {
                    selection
                        .end()
                        .to_point(buffer)
                        .expect("Selection end must be convertible to a Point")
                };
                if let Ok(mut boundaries) = buffer.subword_ends_from_offset_exclusive(end_position)
                {
                    let subword_start = boundaries.next().unwrap_or(end_position);
                    let cursor = map
                        .anchor_before(
                            subword_start
                                .to_display_point(map, ctx)
                                .expect("Subword start must be convertible to a DisplayPoint"),
                            Bias::Right,
                            ctx,
                        )
                        .unwrap_or(Anchor::End);
                    if select {
                        selection.set_head(buffer, cursor);
                    } else {
                        selection.set_selection(Selection::single_cursor(cursor));
                    }
                    selection.goal_start_column = None;
                    selection.goal_end_column = None;
                }
            }
            editor_model.change_selections(new_selections, ctx);
        });
    }

    fn cursor_backward_one_subword(&mut self, select: bool, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let buffer = editor_model.buffer(ctx);
            let map = editor_model.display_map(ctx);

            let mut new_selections = editor_model.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                let start_position = if select {
                    selection
                        .head()
                        .to_point(buffer)
                        .expect("Selection head must be convertible to a Point")
                } else {
                    selection
                        .start()
                        .to_point(buffer)
                        .expect("Selection start must be convertible to a Point")
                };
                if let Ok(mut boundaries) =
                    buffer.subword_backward_starts_from_offset_exclusive(start_position)
                {
                    let subword_start = boundaries.next().unwrap_or(start_position);
                    let cursor = map
                        .anchor_before(
                            subword_start
                                .to_display_point(map, ctx)
                                .expect("Subword start must be convertible to a DisplayPoint"),
                            Bias::Left,
                            ctx,
                        )
                        .unwrap_or(Anchor::Start);
                    if select {
                        selection.set_head(buffer, cursor);
                    } else {
                        selection.set_selection(Selection::single_cursor(cursor));
                    }
                    selection.goal_start_column = None;
                    selection.goal_end_column = None;
                }
            }
            editor_model.change_selections(new_selections, ctx);
        });
    }

    pub fn select_right(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let buffer = editor_model.buffer(ctx);
            let map = editor_model.display_map(ctx);

            let mut new_selections = editor_model.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                let head = selection.head().to_display_point(map, ctx).unwrap();
                let cursor = map
                    .anchor_before(
                        movement::right(map, head, ctx, false).unwrap(),
                        Bias::Right,
                        ctx,
                    )
                    .unwrap();
                selection.set_head(buffer, cursor);
                selection.goal_start_column = None;
                selection.goal_end_column = None;
            }
            editor_model.change_selections(new_selections, ctx);
        });
    }

    /// Determine if an upward navigation command (i.e. `Up` or `PageUp`) should be propagated
    /// to the parent View
    fn should_propagate_upward_navigation(&self, ctx: &mut ViewContext<Self>) -> bool {
        match self.propagate_vertical_navigation_keys {
            PropagateAndNoOpNavigationKeys::Always => true,
            PropagateAndNoOpNavigationKeys::Never => false,
            PropagateAndNoOpNavigationKeys::AtBoundary => self.single_cursor_on_first_row(ctx),
        }
    }

    /// Determine if a downward navigation command (i.e. `Down` or `PageDown`) should be propagated
    /// to the parent View
    fn should_propagate_downward_navigation(&self, ctx: &mut ViewContext<Self>) -> bool {
        match self.propagate_vertical_navigation_keys {
            PropagateAndNoOpNavigationKeys::Always => true,
            PropagateAndNoOpNavigationKeys::Never => false,
            PropagateAndNoOpNavigationKeys::AtBoundary => self.single_cursor_on_last_row(ctx),
        }
    }

    /// Determine if a rightward navigation command (i.e. `Right`) should be propagated
    /// to the parent View
    fn should_propagate_rightward_navigation(&self, ctx: &mut ViewContext<Self>) -> bool {
        match self.propagate_horizontal_navigation_keys {
            PropagateHorizontalNavigationKeys::Always => true,
            PropagateHorizontalNavigationKeys::Never => false,
            PropagateHorizontalNavigationKeys::AtBoundary => self.single_cursor_at_buffer_end(ctx),
        }
    }

    pub fn up(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            if self.should_propagate_upward_navigation(ctx) {
                ctx.emit(Event::Navigate(NavigationKey::Up));
            } else {
                self.move_up(ctx);
            }
        }
    }

    pub fn down(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            if self.should_propagate_downward_navigation(ctx) {
                ctx.emit(Event::Navigate(NavigationKey::Down));
            } else {
                self.move_down(ctx);
            }
        }
    }

    pub fn right(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            if self.should_propagate_rightward_navigation(ctx) {
                ctx.emit(Event::Navigate(NavigationKey::Right));
            }
            self.move_right(/* stop at line end */ false, ctx);
        }
    }

    pub fn single_cursor_to_point(&self, ctx: &AppContext) -> Option<Point> {
        let model = self.editor_model.as_ref(ctx);
        if !model.is_single_cursor_only(ctx) {
            None
        } else {
            let buffer = model.buffer(ctx);
            Some(model.first_selection(ctx).start().to_point(buffer).unwrap())
        }
    }

    /// Return the end position of the first selection, used for positioning argument suggestions in the editor input
    pub fn first_selection_end_to_point(&self, ctx: &AppContext) -> Point {
        let model = self.editor_model.as_ref(ctx);
        let buffer = model.buffer(ctx);
        model.first_selection(ctx).end().to_point(buffer).unwrap()
    }

    pub fn single_cursor_at_buffer_start(&self, ctx: &AppContext) -> bool {
        let buffer = self.editor_model.as_ref(ctx).buffer(ctx);
        self.editor_model.as_ref(ctx).is_single_cursor_only(ctx)
            && self.first_selection(ctx).end().to_point(buffer).unwrap() == Point::new(0, 0)
    }

    /// Check if there is only one cursor and if it is at the end of the editor.
    pub fn single_cursor_at_buffer_end(&self, ctx: &AppContext) -> bool {
        let single_cursor = self.editor_model.as_ref(ctx).is_single_cursor_only(ctx);
        if !single_cursor {
            return false;
        }
        let buffer = self.editor_model.as_ref(ctx).buffer(ctx);
        let max_point = buffer.max_point();
        let cursor = self
            .first_selection(ctx)
            .end()
            .to_point(buffer)
            .expect("There is always at least one selection");
        let at_max_row = max_point.row == cursor.row;
        let at_max_col = max_point.column == cursor.column;
        single_cursor && at_max_row && at_max_col
    }

    /// Whether the cursor appears on the first visual row, taking into account the line's
    /// current soft-wrapping.
    pub fn single_cursor_on_first_row(&self, ctx: &AppContext) -> bool {
        let map = self.editor_model.as_ref(ctx).display_map(ctx);
        if self.editor_model.as_ref(ctx).is_single_cursor_only(ctx) {
            let selection = self.first_selection(ctx);
            let display_point = selection
                .start()
                .to_display_point(map, ctx)
                .expect("Should be able to convert selection to display point");
            map.to_soft_wrap_point(display_point, selection.clamp_direction)
                .map(|soft_wrap_point| soft_wrap_point.row() == 0)
                .unwrap_or(false)
        } else {
            false
        }
    }

    /// Whether the cursor is on the first logical line, regardless of the line's
    /// current soft-wrapping.
    pub fn single_cursor_on_first_line(&self, ctx: &AppContext) -> bool {
        let buffer = self.editor_model.as_ref(ctx).buffer(ctx);
        if self.editor_model.as_ref(ctx).is_single_cursor_only(ctx) {
            let selection = self.first_selection(ctx);
            let point = selection
                .start()
                .to_point(buffer)
                .expect("Should be able to convert selection to point");
            point.row == 0
        } else {
            false
        }
    }

    /// Whether the cursor is on the last visual row, taking into account the line's
    /// current soft-wrapping.
    pub fn single_cursor_on_last_row(&self, ctx: &AppContext) -> bool {
        let map = self.editor_model.as_ref(ctx).display_map(ctx);
        self.editor_model.as_ref(ctx).is_single_cursor_only(ctx)
            && self
                .first_selection(ctx)
                .start()
                .to_display_point(map, ctx)
                .expect("Should be able to convert selection to display point")
                .row()
                == map.max_point(ctx).row()
    }

    /// Whether the cursor is on the last logical line, regardless of the line's
    /// current soft-wrapping.
    pub fn single_cursor_on_last_line(&self, ctx: &AppContext) -> bool {
        let buffer = self.editor_model.as_ref(ctx).buffer(ctx);
        self.editor_model.as_ref(ctx).is_single_cursor_only(ctx)
            && self
                .first_selection(ctx)
                .start()
                .to_point(buffer)
                .expect("Should be able to convert selection to point")
                .row
                == buffer.max_point().row
    }

    /// Returns the line number where the single cursor exists. If there's more than one selection
    /// or the selection is more than just a cursor, returns None.
    pub fn single_cursor_line_index(&self, ctx: &AppContext) -> Option<u32> {
        let model = self.editor_model.as_ref(ctx);
        if model.selections(ctx).len() == 1 {
            let selection = model.first_selection(ctx);
            let buffer = model.buffer(ctx);
            selection.is_cursor_only(buffer).then(|| {
                let cursor = selection.end();
                cursor
                    .to_point(buffer)
                    .expect("cursor should convert to point")
                    .row
            })
        } else {
            None
        }
    }

    pub fn move_up(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.single_line {
            self.change_selections(ctx, |editor_model, ctx| {
                let map = editor_model.display_map(ctx);

                let mut new_selections = editor_model.selections(ctx).clone();
                for selection in new_selections.iter_mut() {
                    let start = selection.start().to_display_point(map, ctx).unwrap();
                    let end = selection.end().to_display_point(map, ctx).unwrap();
                    if start != end {
                        selection.goal_start_column = None;
                        selection.goal_end_column = None;
                    }

                    match map.up(end, selection.goal_end_column, selection.clamp_direction) {
                        Ok(result) => {
                            // We can't assume the display point is always valid because in the very rare case,
                            // two editor actions could be dispatched so close together that the re-render hasn't
                            // started for the first action. This would cause the SoftWrappedState to fall behind
                            // the FoldMap which is updated immediately upon buffer change and subsequently cause
                            // the returned MovementResult to be invalid.
                            //
                            // TODO: We should fix this synchronization issue eventually.
                            let cursor = match map.anchor_before(
                                result.point_and_clamp_direction.point,
                                Bias::Left,
                                ctx,
                            ) {
                                Ok(cursor) => cursor,
                                Err(e) => {
                                    log::warn!("Couldn't convert display point to anchor: {e}");
                                    return;
                                }
                            };

                            selection.set_selection(Selection::single_cursor(cursor));
                            selection.goal_start_column = Some(result.goal_column);
                            selection.goal_end_column = Some(result.goal_column);
                            selection.clamp_direction =
                                result.point_and_clamp_direction.clamp_direction;
                        }
                        Err(err) => {
                            report_error!(err.context("Failed to call DisplayMap#up"));
                        }
                    }
                }
                editor_model.change_selections(new_selections, ctx);
            });
        }
    }

    pub fn select_up(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.single_line {
            self.change_selections(ctx, |editor_model, ctx| {
                let buffer = editor_model.buffer(ctx);
                let map = editor_model.display_map(ctx);
                let mut new_selections = editor_model.selections(ctx).clone();
                for selection in new_selections.iter_mut() {
                    let head = selection
                        .head()
                        .to_display_point(map, ctx)
                        .expect("Should be able to convert Anchor to DisplayPoint");

                    match map.up(head, selection.goal_end_column, selection.clamp_direction) {
                        Ok(result) => {
                            // We can't assume the display point is always valid because in the very rare case,
                            // two editor actions could be dispatched so close together that the re-render hasn't
                            // started for the first action. This would cause the SoftWrappedState to fall behind
                            // the FoldMap which is updated immediately upon buffer change and subsequently cause
                            // the returned MovementResult to be invalid.
                            //
                            // TODO: We should fix this synchronization issue eventually.
                            let cursor = match map.anchor_before(
                                result.point_and_clamp_direction.point,
                                Bias::Left,
                                ctx,
                            ) {
                                Ok(cursor) => cursor,
                                Err(e) => {
                                    log::warn!("Couldn't convert display point to anchor: {e}");
                                    return;
                                }
                            };

                            selection.set_head(buffer, cursor);
                            selection.goal_start_column = Some(result.goal_column);
                            selection.goal_end_column = Some(result.goal_column);
                            selection.clamp_direction =
                                result.point_and_clamp_direction.clamp_direction;
                        }
                        Err(err) => {
                            log::warn!("Failed to call DisplayMap#up {err:?}");
                        }
                    }
                }
                editor_model.change_selections(new_selections, ctx);
            });
        }
    }

    pub fn move_down(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.single_line {
            self.change_selections(ctx, |editor_model, ctx| {
                let map = editor_model.display_map(ctx);

                let mut new_selections = editor_model.selections(ctx).clone();
                for selection in new_selections.iter_mut() {
                    let start = selection.start().to_display_point(map, ctx).unwrap();
                    let end = selection.end().to_display_point(map, ctx).unwrap();
                    if start != end {
                        selection.goal_start_column = None;
                        selection.goal_end_column = None;
                    }
                    match map.down(end, selection.goal_end_column, selection.clamp_direction) {
                        Ok(result) => {
                            // We can't assume the display point is always valid because in the very rare case,
                            // two editor actions could be dispatched so close together that the re-render hasn't
                            // started for the first action. This would cause the SoftWrappedState to fall behind
                            // the FoldMap which is updated immediately upon buffer change and subsequently cause
                            // the returned MovementResult to be invalid.
                            //
                            // TODO: We should fix this synchronization issue eventually.
                            let cursor = match map.anchor_before(
                                result.point_and_clamp_direction.point,
                                Bias::Right,
                                ctx,
                            ) {
                                Ok(cursor) => cursor,
                                Err(e) => {
                                    log::warn!("Couldn't convert display point to anchor: {e}");
                                    return;
                                }
                            };
                            selection.set_selection(Selection::single_cursor(cursor));
                            selection.goal_start_column = Some(result.goal_column);
                            selection.goal_end_column = Some(result.goal_column);
                            selection.clamp_direction =
                                result.point_and_clamp_direction.clamp_direction;
                        }
                        Err(err) => {
                            log::warn!("Failed to call DisplayMap#down {err:?}");
                        }
                    }
                }
                editor_model.change_selections(new_selections, ctx);
            });
        }
    }

    pub fn select_down(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.single_line {
            self.change_selections(ctx, |editor_model, ctx| {
                let buffer = editor_model.buffer(ctx);
                let map = editor_model.display_map(ctx);

                let mut new_selections = editor_model.selections(ctx).clone();
                for selection in new_selections.iter_mut() {
                    let head = selection
                        .head()
                        .to_display_point(map, ctx)
                        .expect("Should be able to convert Anchor to DisplayPoint");
                    match map.down(head, selection.goal_end_column, selection.clamp_direction) {
                        Ok(result) => {
                            // We can't assume the display point is always valid because in the very rare case,
                            // two editor actions could be dispatched so close together that the re-render hasn't
                            // started for the first action. This would cause the SoftWrappedState to fall behind
                            // the FoldMap which is updated immediately upon buffer change and subsequently cause
                            // the returned MovementResult to be invalid.
                            //
                            // TODO: We should fix this synchronization issue eventually.
                            let cursor = match map.anchor_before(
                                result.point_and_clamp_direction.point,
                                Bias::Right,
                                ctx,
                            ) {
                                Ok(cursor) => cursor,
                                Err(e) => {
                                    log::warn!("Couldn't convert display point to anchor: {e}");
                                    return;
                                }
                            };

                            selection.set_head(buffer, cursor);
                            selection.goal_start_column = Some(result.goal_column);
                            selection.goal_end_column = Some(result.goal_column);
                            selection.clamp_direction =
                                result.point_and_clamp_direction.clamp_direction;
                        }
                        Err(err) => {
                            report_error!(err.context("Failed to call DisplayMap#down"))
                        }
                    }
                }
                editor_model.change_selections(new_selections, ctx);
            });
        }
    }

    pub fn select_all(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            editor_model
                .select_ranges_by_offset([CharOffset::zero()..editor_model.buffer_len(ctx)], ctx)
                .unwrap();
        });
    }

    pub fn undo(&mut self, ctx: &mut ViewContext<Self>) {
        self.editor_model.update(ctx, |editor_model, ctx| {
            editor_model.undo(ctx);
        });
    }

    pub fn redo(&mut self, ctx: &mut ViewContext<Self>) {
        self.editor_model.update(ctx, |editor_model, ctx| {
            editor_model.redo(ctx);
        });
    }

    fn expand_selections(&mut self, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let buffer = editor_model.buffer(ctx);
            let map = editor_model.display_map(ctx);
            let mut new_selections = Vec::new();
            for selection in editor_model.selections(ctx).clone().iter_mut() {
                // Keep non-cursor-only selections the same
                if !selection.is_cursor_only(buffer) {
                    new_selections.push(selection.clone());
                    continue;
                }
                let start = selection.tail();
                let position = start
                    .to_display_point(map, ctx)
                    .unwrap()
                    .to_buffer_point(map, Bias::Left, ctx)
                    .unwrap();
                let word_end = buffer
                    .word_ends_from_offset_inclusive(position)
                    .unwrap()
                    .next();
                let word_start = buffer
                    .word_starts_backward_from_offset_inclusive(position)
                    .unwrap()
                    .next();
                if let (Some(start), Some(end)) = (word_start, word_end) {
                    new_selections.push(LocalSelection {
                        selection: Selection {
                            start: buffer.anchor_before(start).unwrap(),
                            end: buffer.anchor_before(end).unwrap(),
                            reversed: false,
                        },
                        clamp_direction: selection.clamp_direction,
                        goal_start_column: None,
                        goal_end_column: None,
                    });
                }
            }

            if let Ok(new_selections) = Vec1::try_from_vec(new_selections) {
                editor_model.change_selections(new_selections, ctx);
            }
        });
    }

    fn select_next_occurrence(
        &mut self,
        ctx: &mut ViewContext<Self>,
        selected_text: String,
        start_offset: ByteOffset,
    ) {
        self.change_selections(ctx, |editor_model, ctx| {
            let buffer = editor_model.buffer(ctx);
            let map = editor_model.display_map(ctx);

            let buffer_text: String = buffer.text();
            let len = selected_text.len();
            let (before, after): (Vec<_>, Vec<_>) = buffer_text
                .match_indices(selected_text.as_str())
                .partition(|s| s.0 < start_offset.as_usize());

            for (i, _) in after.iter().chain(before.iter()) {
                let Ok(start) = buffer
                    .point_for_offset(ByteOffset::from(*i))
                    .and_then(|p| p.to_display_point(map, ctx))
                else {
                    log::warn!("Failed to convert start offset to point");
                    return;
                };
                let Ok(anchor_before_start) = map.anchor_before(start, Bias::Left, ctx) else {
                    log::warn!("Failed to find anchor before start point");
                    return;
                };

                let Ok(end) = buffer
                    .point_for_offset(ByteOffset::from(*i + len))
                    .and_then(|p| p.to_display_point(map, ctx))
                else {
                    log::warn!("Failed to convert end offset to point");
                    return;
                };
                let Ok(anchor_before_end) = map.anchor_before(end, Bias::Left, ctx) else {
                    log::warn!("Failed to find anchor before end point");
                    return;
                };

                if editor_model
                    .local_selections_intersecting_range(start..end, ctx)
                    .next()
                    .is_none()
                {
                    let selection = LocalSelection {
                        selection: Selection {
                            start: anchor_before_start,
                            end: anchor_before_end,
                            reversed: false,
                        },
                        clamp_direction: Default::default(),
                        goal_start_column: None,
                        goal_end_column: None,
                    };

                    let ix = editor_model.selection_insertion_index(selection.start(), ctx);
                    let mut new_selections = editor_model.selections(ctx).clone();
                    new_selections.insert(ix, selection);
                    editor_model.change_selections(new_selections, ctx);
                    return;
                }
            }

            // If we get there, then we didn't find a next occurrence.
            log::warn!("Unable to select next occurrence");
        });
    }

    fn should_select_next_occurrence(&mut self, ctx: &ViewContext<Self>) -> bool {
        let buffer = self.editor_model.as_ref(ctx).buffer(ctx);
        let mut last_selected_text: Option<String> = None;

        for selection in self.selections(ctx).iter() {
            if selection.is_cursor_only(buffer) {
                return false;
            }
            let start = selection.start().to_char_offset(buffer).unwrap();
            let end = selection.end().to_char_offset(buffer).unwrap();
            let selected_text: String = buffer
                .chars_at(start)
                .unwrap()
                .take(end.as_usize() - start.as_usize())
                .collect();
            if last_selected_text.is_none() {
                last_selected_text = Some(selected_text.clone());
            }
            if last_selected_text != Some(selected_text) {
                return false;
            }
        }
        last_selected_text.is_some()
    }

    pub fn add_next_occurrence(&mut self, ctx: &mut ViewContext<Self>) {
        let buffer = self.editor_model.as_ref(ctx).buffer(ctx);
        if self.should_select_next_occurrence(ctx) {
            let first_selection = self.first_selection(ctx);
            let start_offset = first_selection.start().to_byte_offset(buffer).unwrap();
            let start = first_selection.start().to_char_offset(buffer).unwrap();
            let end = first_selection.end().to_char_offset(buffer).unwrap();
            let selected_text: String = buffer
                .chars_at(start)
                .unwrap()
                .take(end.as_usize() - start.as_usize())
                .collect();

            self.select_next_occurrence(ctx, selected_text, start_offset);
        } else {
            self.expand_selections(ctx);
        }
    }

    /// Adds a cursor above/below all existing selections.
    ///
    /// For non-empty selections [s..e], the characters above/below [s..e] (if any) become
    /// a new selection [s'..e'] and the new cursor is anchored to the end of e'.
    fn add_cursor(&mut self, direction: NewCursorDirection, ctx: &mut ViewContext<Self>) {
        self.change_selections(ctx, |editor_model, ctx| {
            let map = editor_model.display_map(ctx);
            let mut new_selections = Vec::new();

            for selection in editor_model.selections(ctx).iter() {
                // Find the endpoints of selection as DPs.
                let start = selection
                    .start()
                    .to_display_point(map, ctx)
                    .expect("Should be able to get start point of selection.");
                let end = selection
                    .end()
                    .to_display_point(map, ctx)
                    .expect("Should be able to get end point of selection");

                // Determine the new start and end cursors after the vertical move.
                let start_result = match direction.move_cursor(
                    map,
                    start,
                    selection.goal_start_column,
                    selection.clamp_direction,
                ) {
                    Some(values) => values,
                    None => continue,
                };
                let end_result = match direction.move_cursor(
                    map,
                    end,
                    selection.goal_end_column,
                    selection.clamp_direction,
                ) {
                    Some(values) => values,
                    None => continue,
                };

                let new_start_cursor = map
                    .anchor_before(
                        start_result.point_and_clamp_direction.point,
                        Bias::Left,
                        ctx,
                    )
                    .expect("Should be able to get new start cursor based on new start point.");
                let new_end_cursor = map
                    .anchor_before(end_result.point_and_clamp_direction.point, Bias::Left, ctx)
                    .expect("Should be able to get new end cursor based on new end point");

                // Create a new selection at (new_start, new_end).
                new_selections.push(LocalSelection {
                    selection: Selection {
                        start: new_start_cursor,
                        end: new_end_cursor,
                        reversed: selection.reversed(),
                    },
                    clamp_direction: end_result.point_and_clamp_direction.clamp_direction,
                    goal_start_column: Some(start_result.goal_column),
                    goal_end_column: Some(end_result.goal_column),
                });
            }

            // Add all the new selections to the existing selections.
            for new_selection in new_selections {
                let ix = editor_model.selection_insertion_index(new_selection.start(), ctx);
                let mut new_selections = editor_model.selections(ctx).clone();
                new_selections.insert(ix, new_selection);
                editor_model.change_selections(new_selections, ctx);
            }
        });
    }

    pub fn page_up(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            if self.should_propagate_upward_navigation(ctx) {
                ctx.emit(Event::Navigate(NavigationKey::PageUp));
            } else {
                self.move_page_up(ctx);
            }
        }
    }

    pub fn page_down(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_edit(ctx) {
            if self.should_propagate_downward_navigation(ctx) {
                ctx.emit(Event::Navigate(NavigationKey::PageDown));
            } else {
                self.move_page_down(ctx);
            }
        }
    }

    pub fn move_page_up(&mut self, ctx: &mut ViewContext<Self>) {
        let model = self.model().as_ref(ctx);
        let buffer = model.buffer(ctx);
        let first_cursor_col: u32 = model
            .first_selection(ctx)
            .start()
            .to_point(buffer)
            .unwrap()
            .column;
        let first_row_new_col: u32 = cmp::min(buffer.line_len(0).unwrap(), first_cursor_col);

        // Single Cursor
        if model.is_single_cursor_only(ctx) {
            let cursor_row = model
                .first_selection(ctx)
                .start()
                .to_point(buffer)
                .unwrap()
                .row;

            if cursor_row == 0 {
                // Move to line start
                self.move_to_line_start(ctx)
            } else {
                // Move to col min(first_cursor_col, length of first row) on first row
                self.change_selections(ctx, |editor_model, ctx| {
                    let point = Point::new(0, first_row_new_col);
                    editor_model.reset_selections_to_point(&point, ctx);
                });
            }
            return;
        }

        // Multiple Cursors
        if model.selections(ctx).iter().all(|selection| {
            let is_cursor = selection.is_cursor_only(buffer);
            let same_col = selection.start().to_point(buffer).unwrap().column == first_cursor_col;
            is_cursor && same_col
        }) {
            // Multiple cursors, all same column
            self.change_selections(ctx, |editor_model, ctx| {
                let point = Point::new(0, first_row_new_col);
                editor_model.reset_selections_to_point(&point, ctx);
            });
        } else {
            // Multiple cursors, different columns
            self.cursor_top(ctx);
        }
    }

    pub fn move_page_down(&mut self, ctx: &mut ViewContext<Self>) {
        let model = self.model().as_ref(ctx);
        let buffer = model.buffer(ctx);
        let map = self.editor_model.as_ref(ctx).display_map(ctx);
        let last_row = map.max_point(ctx).row();
        let first_cursor_col: u32 = model
            .first_selection(ctx)
            .start()
            .to_point(buffer)
            .unwrap()
            .column;
        let last_row_new_col: u32 = cmp::min(buffer.line_len(last_row).unwrap(), first_cursor_col);

        // Single Cursor
        if model.is_single_cursor_only(ctx) {
            let cursor_row = model
                .first_selection(ctx)
                .start()
                .to_point(buffer)
                .unwrap()
                .row;

            if cursor_row == last_row {
                // Move to line end
                self.move_to_line_end(ctx)
            } else {
                // Move to col min(first_cursor_col, length of last row) on last row
                self.change_selections(ctx, |editor_model, ctx| {
                    let point = Point::new(last_row, last_row_new_col);
                    editor_model.reset_selections_to_point(&point, ctx);
                });
            }
            return;
        }

        // Multiple Cursors
        if model.selections(ctx).iter().all(|selection| {
            let is_cursor = selection.is_cursor_only(buffer);
            let same_col = selection.start().to_point(buffer).unwrap().column == first_cursor_col;
            is_cursor && same_col
        }) {
            // Multiple cursors, all same column
            self.change_selections(ctx, |editor_model, ctx| {
                let point = Point::new(last_row, last_row_new_col);
                editor_model.reset_selections_to_point(&point, ctx);
            });
        } else {
            // Multiple cursors, different column
            self.cursor_bottom(ctx);
        }
    }

    pub fn line_len(&self, display_row: u32, ctx: &AppContext) -> Result<u32> {
        self.editor_model.as_ref(ctx).line_len(display_row, ctx)
    }

    pub fn max_point(&self, ctx: &AppContext) -> DisplayPoint {
        self.editor_model.as_ref(ctx).max_point(ctx)
    }

    /// Font size of this editor.
    pub fn font_size(&self, appearance: &Appearance) -> f32 {
        self.text_options
            .font_size_override
            .unwrap_or_else(|| appearance.monospace_font_size())
    }

    /// Font family of this editor.
    pub fn font_family(&self, appearance: &Appearance) -> FamilyId {
        if self.is_password {
            return appearance.password_font_family();
        }
        self.text_options
            .font_family_override
            .unwrap_or_else(|| appearance.monospace_font_family())
    }

    /// Font family of this editor's placeholder text.
    pub fn placeholder_font_family(&self, appearance: &Appearance) -> FamilyId {
        self.text_options
            .font_family_override
            .unwrap_or_else(|| appearance.monospace_font_family())
    }

    pub fn line_height_ratio(&self, appearance: &Appearance) -> f32 {
        if self.use_settings_line_height_ratio {
            return appearance.line_height_ratio();
        }
        DEFAULT_UI_LINE_HEIGHT_RATIO
    }

    fn font_properties(&self, appearance: &Appearance) -> Properties {
        self.text_options
            .font_properties_override
            .unwrap_or_else(|| Properties::default().weight(appearance.monospace_font_weight()))
    }

    fn text_colors(&self, appearance: &Appearance) -> TextColors {
        self.text_options
            .text_colors_override
            .clone()
            .unwrap_or_else(|| TextColors::from_appearance(appearance))
    }

    pub fn line_height(&self, font_cache: &FontCache, appearance: &Appearance) -> f32 {
        // Copy the font family ID so that it can be moved into the closure.
        let font_family = self.font_family(appearance);
        match self.baseline_position_computation_method {
            BaselinePositionComputationMethod::Grid => grid_cell_dimensions(
                font_cache,
                font_family,
                self.font_size(appearance),
                self.line_height_ratio(appearance),
            )
            .y(),
            BaselinePositionComputationMethod::Default => font_cache.line_height(
                self.font_size(appearance),
                self.line_height_ratio(appearance),
            ),
        }
    }

    pub fn em_width(&self, font_cache: &FontCache, appearance: &Appearance) -> f32 {
        font_cache.em_width(self.font_family(appearance), self.font_size(appearance))
    }

    pub fn placeholder_text_exists(&self) -> bool {
        !self.placeholder_texts.is_empty()
    }

    fn next_blink_epoch(&mut self) -> usize {
        self.blink_epoch += 1;
        self.blink_epoch
    }

    /// Mark the cursor as visible and schedule the next cursor blink at the right interval.
    fn reset_cursor_blink_timer(&mut self, ctx: &mut ViewContext<Self>) {
        self.cursors_visible = true;
        ctx.notify();

        let epoch = self.next_blink_epoch();
        let _ = ctx.spawn(
            async move {
                Timer::after(CURSOR_BLINK_INTERVAL).await;
                epoch
            },
            Self::blink_cursors,
        );
    }

    /// Returns a snapshot of the editor.
    pub fn snapshot_model(&self, ctx: &AppContext) -> EditorSnapshot {
        self.editor_model.as_ref(ctx).as_snapshot(ctx)
    }

    fn blink_cursors(&mut self, epoch: usize, ctx: &mut ViewContext<Self>) {
        let cursor_blink = &EditorSettings::as_ref(ctx).cursor_blink;
        if epoch == self.blink_epoch
            && self.focused_in_active_window(ctx)
            && (
                // Allow this method to recurse on itself (and continue blinking)
                // if blinking is enabled. Otherwise, it can only recurse if
                // cursors_visible is false, then it may recurse just 1 more time
                // to make the cursor visible again and remain that way. We need
                // to allow that in case blinking gets disabled while
                // cursors_visible is false
                cursor_blink.value() == &CursorBlink::Enabled || !self.cursors_visible
            )
        {
            self.cursors_visible = !self.cursors_visible;
            ctx.notify();

            let epoch = self.next_blink_epoch();
            let _ = ctx.spawn(
                async move {
                    Timer::after(CURSOR_BLINK_INTERVAL).await;
                    epoch
                },
                Self::blink_cursors,
            );
        }
    }

    fn focused_in_active_window(&self, ctx: &AppContext) -> bool {
        let active = self
            .windowing_state_handle
            .as_ref(ctx)
            .state()
            .active_window;
        Some(self.window_id) == active && self.focused
    }

    fn should_draw_cursors(&self, ctx: &AppContext) -> bool {
        self.cursors_visible && self.focused_in_active_window(ctx) && self.can_edit(ctx)
    }

    fn handle_model_event(
        &mut self,
        _: ModelHandle<EditorModel>,
        event: &EditorModelEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            EditorModelEvent::Edited { edit_origin } => ctx.emit(Event::Edited(*edit_origin)),
            EditorModelEvent::BufferReplaced => ctx.emit(Event::BufferReplaced),
            EditorModelEvent::SelectionsChanged => ctx.emit(Event::SelectionChanged),
            EditorModelEvent::ShellCut(text) => {
                self.internal_clipboard.clone_from(text);
                self.internal_clipboard.shrink_to_fit();
            }
            // In these cases, we just need to re-render, which is handled
            // by the invalidation below.
            EditorModelEvent::DisplayMapUpdated | EditorModelEvent::StylesUpdated => {}
        }

        // The editor model can change even when we aren't focused, so we only want to reset the
        // blink timer when we are active
        if self.focused_in_active_window(ctx) {
            self.reset_cursor_blink_timer(ctx);
        }
        *self.autoscroll_requested.lock() = true;
        ctx.notify();
    }

    fn first_selection<'a, C: ModelAsRef>(&'a self, ctx: &'a C) -> &'a LocalSelection {
        self.editor_model.as_ref(ctx).first_selection(ctx)
    }

    fn selections<'a, C: ModelAsRef>(&'a self, ctx: &'a C) -> &'a Vec1<LocalSelection> {
        self.editor_model.as_ref(ctx).selections(ctx)
    }

    pub fn num_selections<C: ModelAsRef>(&self, ctx: &C) -> usize {
        self.selections(ctx).len()
    }

    pub fn any_selections_span_entire_buffer<C: ModelAsRef>(&self, ctx: &C) -> bool {
        self.editor_model
            .as_ref(ctx)
            .any_selections_span_entire_buffer(ctx)
    }

    /// For every selection, we return an iterator of the chars preceding that selection.
    pub fn chars_preceding_selections<'a, C: ModelAsRef>(
        &'a self,
        ctx: &'a C,
    ) -> impl Iterator<Item = Chars<'a>> + 'a {
        self.editor_model
            .as_ref(ctx)
            .chars_preceding_selections(ctx)
    }

    pub fn is_single_cursor_only<C: ModelAsRef>(&self, ctx: &C) -> bool {
        self.editor_model.as_ref(ctx).is_single_cursor_only(ctx)
    }

    /// Commits the currently composed text from the IME (if there is any) to properly handle one of the following:
    /// - a new selection
    /// - clicking outside of the editor
    fn maybe_commit_incomplete_ime_text(&mut self, ctx: &mut ViewContext<Self>) {
        let marked_text_state = self
            .editor_model
            .read(ctx, |editor_model, ctx| editor_model.marked_text_state(ctx));
        if matches!(marked_text_state, MarkedTextState::Inactive) {
            return;
        }
        self.edit(
            ctx,
            Edits::new().with_update_buffer(
                PlainTextEditorViewAction::UpdateMarkedText,
                EditOrigin::UserTyped,
                |editor_model, ctx| {
                    editor_model.commit_incomplete_marked_text(ctx);
                },
            ),
        );
        ctx.notify();
    }

    fn set_marked_text(
        &mut self,
        marked_text: &str,
        selected_range: &Range<usize>,
        ctx: &mut ViewContext<Self>,
    ) {
        let cursor_colors = (self.get_cursor_colors_fn)(ctx);
        self.edit(
            ctx,
            Edits::new().with_update_buffer_options(
                PlainTextEditorViewAction::UpdateMarkedText,
                EditOrigin::UserTyped,
                // We don't want marked text updates to be sent to be a CRDT operation.
                UpdateBufferOption::IsEphemeral,
                |editor_model, ctx| {
                    editor_model.update_marked_text(
                        marked_text,
                        Some(TextStyle::new().with_underline_color(cursor_colors.cursor.into())),
                        selected_range,
                        ctx,
                    );
                },
            ),
        );
        ctx.notify();
    }

    fn clear_marked_text(&mut self, ctx: &mut ViewContext<Self>) {
        self.editor_model.update(ctx, |editor_model, ctx| {
            editor_model.clear_marked_text(ctx);
        });
    }
}

/// Try to convert display point to an anchor. If it is not possible, clamp to anchoring at end of buffer.
fn display_point_to_anchor_clamped(
    editor_model: &EditorModel,
    position: DisplayPoint,
    bias: Bias,
    ctx: &AppContext,
) -> Anchor {
    match editor_model
        .display_map(ctx)
        .anchor_before(position, bias, ctx)
    {
        Ok(anchor) => anchor,
        Err(e) => {
            log::warn!("Should be able to get anchor {e}");
            Anchor::End
        }
    }
}

#[derive(Debug)]
pub enum Event {
    Activate,
    Edited(EditOrigin),
    Blurred,
    Focused,
    SelectionChanged,
    Navigate(NavigationKey),
    Enter,
    ShiftEnter,
    AltEnter,
    CtrlEnter,
    /// Attempt to remove text when there is a single cursor placed at the beginning of the editor
    /// buffer.
    BackspaceAtBeginningOfBuffer,
    BackspaceOnEmptyBuffer,
    BufferReplaced,
    CmdUpOnFirstRow,
    Copy,
    Escape,
    ClearParentSelections,
    CmdEnter,
    /// Signifies that something has been pasted into the view
    Paste,
    MiddleClickPaste,
    /// Emitted when the 'delete all left' keybinding is triggered (cmd-backspace on mac).
    DeleteAllLeft,
}

impl Entity for EditorView {
    type Event = Event;
}

impl TypedActionView for EditorView {
    type Action = EditorAction;

    fn action_accessibility_contents(
        &mut self,
        action: &EditorAction,
        ctx: &mut ViewContext<Self>,
    ) -> ActionAccessibilityContent {
        match action {
            EditorAction::UserInsert(text) => ActionAccessibilityContent::Custom(
                AccessibilityContent::new_without_help(text.to_string(), WarpA11yRole::UserAction),
            ),
            EditorAction::SelectLeft
            | EditorAction::SelectToLineEnd
            | EditorAction::SelectLine(_)
            | EditorAction::SelectLeftByWord
            | EditorAction::SelectLeftBySubword
            | EditorAction::SelectToLineStart
            | EditorAction::Select(_)
            | EditorAction::SelectUp
            | EditorAction::SelectAll
            | EditorAction::SelectDown
            | EditorAction::SelectWord(_)
            | EditorAction::SelectRight
            | EditorAction::SelectRightByWord
            | EditorAction::SelectRightBySubword
            | EditorAction::MoveToLineStart
            | EditorAction::MoveToParagraphStart
            | EditorAction::MoveToBufferStart
            | EditorAction::MoveToLineEnd
            | EditorAction::MoveToParagraphEnd
            | EditorAction::MoveToBufferEnd
            | EditorAction::Left
            | EditorAction::Right
            | EditorAction::MoveToVisualLineStart
            | EditorAction::MoveToVisualLineEnd
            | EditorAction::MoveForwardOneWord
            | EditorAction::MoveBackwardOneWord
            | EditorAction::MoveForwardOneSubword
            | EditorAction::MoveBackwardOneSubword
            | EditorAction::Delete
            | EditorAction::Backspace => ActionAccessibilityContent::Empty,
            EditorAction::Paste => {
                ActionAccessibilityContent::Custom(AccessibilityContent::new_without_help(
                    format!("Pasting: {}", self.clipboard_content(ctx)),
                    WarpA11yRole::UserAction,
                ))
            }
            _ => ActionAccessibilityContent::from_debug(),
        }
    }

    fn handle_action(&mut self, action: &EditorAction, ctx: &mut ViewContext<Self>) {
        use EditorAction::*;
        match action {
            Scroll(position) => self.scroll(*position, ctx),
            Select(action) => self.select(action, ctx),
            UserInsert(text) => self.user_insert(text.as_ref(), ctx),
            Tab => self.tab(ctx),
            ShiftTab => self.shift_tab(ctx),
            Copy => self.copy(ctx),
            Cut => self.cut(ctx),
            Paste => self.paste(ctx),
            MiddleClickPaste => self.middle_click_paste(ctx),
            Yank => self.yank(ctx),
            Newline => self.newline(ctx),
            ShiftEnter => self.shift_enter(ctx),
            CtrlEnter => self.ctrl_enter(ctx),
            AltEnter => self.alt_enter(ctx),
            Enter => self.enter(ctx),
            CutWordLeft => self.cut_word_left(ctx),
            CutWordRight => self.cut_word_right(ctx),
            DeleteWordLeft => self.delete_word_left(ctx),
            DeleteWordRight => self.delete_word_right(ctx),
            CutAllLeft => self.delete_all(CutDirection::Left, true /* cut */, ctx),
            CutAllRight => self.delete_all(CutDirection::Right, true /* cut */, ctx),
            DeleteAllLeft => {
                self.delete_all(CutDirection::Left, false /* cut */, ctx);
                ctx.emit(Event::DeleteAllLeft);
            }
            DeleteAllRight => self.delete_all(CutDirection::Right, false /* cut */, ctx),
            Delete => self.delete(ctx),
            Backspace => self.backspace(ctx),
            Escape => self.escape(ctx),
            Up => self.up(ctx),
            Down => self.down(ctx),
            Left => self.move_left(/* stop at line start */ false, ctx),
            Right => self.right(ctx),
            MoveToVisualLineStart => self.move_to_visual_line_start(ctx),
            MoveToVisualLineEnd => self.move_to_visual_line_end(ctx),
            PageUp => self.page_up(ctx),
            PageDown => self.page_down(ctx),
            CmdUp => self.cmd_up(ctx),
            CmdDown => self.cursor_bottom(ctx),
            ClearAndCopyLines => self.clear_and_copy_lines(ctx),
            MoveToLineStart => self.move_to_line_start(ctx),
            MoveToParagraphStart => self.move_to_paragraph_start(ctx),
            MoveToBufferStart => self.move_to_buffer_start(ctx),
            SelectToLineStart => self.select_to_line_start(ctx),
            MoveToLineEnd => self.move_to_line_end(ctx),
            MoveToParagraphEnd => self.move_to_paragraph_end(ctx),
            MoveToBufferEnd => self.move_to_buffer_end(ctx),
            SelectToLineEnd => self.select_to_line_end(ctx),
            MoveToAndSelectBufferStart => self.move_and_select_to_buffer_start(ctx),
            MoveToAndSelectBufferEnd => self.move_and_select_to_buffer_end(ctx),
            MoveForwardOneWord => self.cursor_forward_one_word(false /* select */, ctx),
            MoveBackwardOneWord => self.cursor_backward_one_word(false /* select */, ctx),
            MoveForwardOneSubword => self.cursor_forward_one_subword(false /* select */, ctx),
            MoveBackwardOneSubword => {
                self.cursor_backward_one_subword(false /* select */, ctx)
            }
            SelectUp => self.select_up(ctx),
            SelectDown => self.select_down(ctx),
            SelectLeft => self.select_left(ctx),
            SelectRight => self.select_right(ctx),
            SelectWord(position) => self.select_word(position, ctx),
            SelectLine(position) => self.select_line(position, ctx),
            SelectAll => self.select_all(ctx),
            SelectRightByWord => self.cursor_forward_one_word(true /* select */, ctx),
            SelectLeftByWord => self.cursor_backward_one_word(true /* select */, ctx),
            SelectRightBySubword => self.cursor_forward_one_subword(true /* select */, ctx),
            SelectLeftBySubword => self.cursor_backward_one_subword(true /* select */, ctx),
            AddNextOccurrence => self.add_next_occurrence(ctx),
            Undo => self.undo(ctx),
            Redo => self.redo(ctx),
            Focus => {
                ctx.emit(Event::Focused);
                ctx.focus_self();
            }
            ClearParentSelections => {
                self.maybe_commit_incomplete_ime_text(ctx);
                if self.can_select(ctx) {
                    ctx.emit(Event::ClearParentSelections)
                }
            }
            CmdEnter => {
                if self.can_edit(ctx) {
                    ctx.emit(Event::CmdEnter)
                }
            }
            AddCursorAbove => self.add_cursor(NewCursorDirection::Up, ctx),
            AddCursorBelow => self.add_cursor(NewCursorDirection::Down, ctx),
            ShowCharacterPalette => {
                if self.can_edit(ctx) {
                    ctx.open_character_palette();
                }
            }
            ImeCommit(text) => self.ime_commit(text, ctx),
            SetMarkedText {
                marked_text,
                selected_range,
            } => self.set_marked_text(marked_text, selected_range, ctx),
            ClearMarkedText => self.clear_marked_text(ctx),
        }

        if self.is_focused() {
            if action.should_report_active_cursor_position_updated() {
                ctx.report_active_cursor_position_update();
            }
            if action.is_new_selection() {
                SelectionSettings::handle(ctx).update(ctx, |selection_settings, ctx| {
                    selection_settings.maybe_write_to_linux_selection_clipboard(
                        |ctx| ClipboardContent::plain_text(self.selected_text(ctx)),
                        ctx,
                    );
                });
            }
        }
    }
}

impl View for EditorView {
    fn ui_name() -> &'static str {
        "EditorView"
    }

    fn render(&self, ctx: &AppContext) -> Box<dyn Element> {
        let soft_wrap_state = self.model().as_ref(ctx).display_map(ctx).soft_wrap_state();

        let appearance = Appearance::as_ref(ctx);
        let text_colors = self.text_colors(appearance);

        let editor_decorator_elements = self
            .render_decorator_elements
            .as_ref()
            .map(|f| f(ctx))
            .unwrap_or_default();

        let view_snapshot = self.snapshot(ctx);
        let scroll_state = self.into();

        let local_selection_data = LocalDrawableSelectionData {
            colors: (self.get_cursor_colors_fn)(ctx),
            should_draw_cursors: self.should_draw_cursors(ctx),
        };

        let editor_element = EditorElement::new(
            view_snapshot,
            scroll_state,
            self.soft_wrap,
            soft_wrap_state,
            self.placeholder_soft_wrap,
            text_colors,
            editor_decorator_elements,
            local_selection_data,
            self.cursor_display_override,
        );

        Hoverable::new(self.hover_handle.clone(), |_state| editor_element.finish())
            .with_cursor(Cursor::IBeam)
            .finish()
    }

    fn keymap_context(&self, ctx: &AppContext) -> warpui::keymap::Context {
        let mut context = Self::default_keymap_context();

        if self.single_cursor_at_buffer_end(ctx) {
            context.set.insert("EditorView_SingleCursorBufferEnd");
        }

        if let Some(modifier) = &self.keymap_context_modifier {
            modifier(&mut context, ctx);
        }

        context
    }

    fn on_window_transferred(
        &mut self,
        _source_window_id: WindowId,
        target_window_id: WindowId,
        _ctx: &mut ViewContext<Self>,
    ) {
        self.window_id = target_window_id;
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            self.focused = true;
            self.blink_cursors(self.blink_epoch, ctx);
            if self.select_all_on_focus {
                self.select_all(ctx);
            }
            ctx.report_active_cursor_position_update();
            ctx.notify();
        }
    }

    fn on_blur(&mut self, blur_ctx: &BlurContext, ctx: &mut ViewContext<Self>) {
        if blur_ctx.is_self_blurred() {
            self.focused = false;
            self.cursors_visible = false;
            if self.clear_selections_on_blur {
                self.clear_selections(ctx);
            }

            self.maybe_commit_incomplete_ime_text(ctx);
            ctx.emit(Event::Blurred);
            ctx.notify();
        }
    }

    fn active_cursor_position(&self, ctx: &ViewContext<Self>) -> Option<CursorInfo> {
        let cursor_id = position_id_for_cursor(ctx.view_id());
        let appearance = Appearance::as_ref(ctx);
        let font_size = self.font_size(appearance);
        ctx.element_position_by_id(cursor_id)
            .map(|position| CursorInfo {
                position,
                font_size,
            })
    }
}

/// Returns the canonical position ID for a cached point in an editor.
pub fn position_id_for_cached_point(editor_view_id: EntityId, position_id: &str) -> String {
    format!("editor_{editor_view_id}:{position_id}")
}

/// Returns the canonical position ID for the cursor in an editor.
pub fn position_id_for_cursor(editor_view_id: EntityId) -> String {
    format!("editor:cursor_{editor_view_id}")
}

/// Returns the canonical position ID for the first cursor in an editor.
/// Necessary because the above `position_id_for_cursor` refers to the
/// last cursor position, when there are multiple.
pub fn position_id_for_first_cursor(editor_view_id: EntityId) -> String {
    format!("editor:first_cursor_{editor_view_id}")
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Deserialize,
    Serialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(description = "Whether the cursor blinks.", rename_all = "snake_case")]
pub enum CursorBlink {
    #[default]
    Enabled,
    Disabled,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Deserialize,
    Serialize,
    Sequence,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(description = "Visual style of the cursor.", rename_all = "snake_case")]
pub enum CursorDisplayType {
    #[default]
    Bar,
    Block,
    Underline,
}

impl CursorDisplayType {
    pub fn nth(index: usize) -> Option<Self> {
        all::<Self>().nth(index)
    }

    pub fn to_index(&self) -> usize {
        all::<Self>()
            .position(|v| v == *self)
            .expect("Cursor display type not found in Sequence!")
    }
}

pub use editor_settings::{EditorSettings, EditorSettingsChangedEvent};
pub use selection_settings::{SelectionSettings, SelectionSettingsChangedEvent};

// Each settings group lives in its own module: the macro expands to imports that would clash.
mod editor_settings {
    use settings::SupportedPlatforms;
    use settings::macros::define_settings_group;

    use super::{CursorBlink, CursorDisplayType};

    define_settings_group!(EditorSettings, settings: [
        cursor_blink: CursorBlinkEnabled {
            type: CursorBlink,
            default: CursorBlink::default(),
            supported_platforms: SupportedPlatforms::ALL,
            surface: settings::SettingSurfaces::GUI,
            private: false,
            storage_key: "CursorBlink",
            toml_path: "appearance.cursor.cursor_blink",
            description: "Whether the cursor blinks.",
        },
        cursor_display_type: CursorDisplayState {
            type: CursorDisplayType,
            default: CursorDisplayType::default(),
            supported_platforms: SupportedPlatforms::ALL,
            surface: settings::SettingSurfaces::GUI,
            private: false,
            storage_key: "CursorDisplayType",
            toml_path: "appearance.cursor.cursor_display_type",
            description: "The visual style of the cursor.",
        },
        autocomplete_symbols: AutocompleteSymbols {
            type: bool,
            default: true,
            supported_platforms: SupportedPlatforms::ALL,
            surface: settings::SettingSurfaces::GUI,
            private: false,
            toml_path: "text_editing.autocomplete_symbols",
            description: "Whether matching symbols like brackets and quotes are auto-completed.",
        },
    ]);
}

mod selection_settings {
    use settings::macros::define_settings_group;
    use settings::{Setting as _, SupportedPlatforms};
    use warpui::AppContext;
    use warpui::clipboard::ClipboardContent;

    define_settings_group!(SelectionSettings, settings: [
        linux_selection_clipboard: LinuxSelectionClipboard {
            type: bool,
            default: true,
            supported_platforms: SupportedPlatforms::LINUX,
            surface: settings::SettingSurfaces::GUI,
            private: false,
            toml_path: "system.linux_selection_clipboard",
            description: "Whether the Linux primary selection clipboard is used.",
        },
        middle_click_paste_enabled: MiddleClickPasteEnabled {
            type: bool,
            default: true,
            supported_platforms: SupportedPlatforms::OR(
                SupportedPlatforms::WINDOWS.into(),
                SupportedPlatforms::MAC.into()
            ),
            surface: settings::SettingSurfaces::GUI,
            private: false,
            toml_path: "text_editing.middle_click_paste_enabled",
            description: "Whether middle-click pastes from the clipboard.",
        }
    ]);

    impl SelectionSettings {
        /// Returns whether honoring the Linux primary selection clipboard is enabled. On non-linux
        /// platforms this always returns false.
        pub fn linux_selection_clipboard_enabled(&self) -> bool {
            *self.linux_selection_clipboard.value()
                && self
                    .linux_selection_clipboard
                    .is_supported_on_current_platform()
        }

        /// Writes the selected content to the user's primary selection clipboard. On non-Linux
        /// platforms this is a noop.
        pub fn maybe_write_to_linux_selection_clipboard(
            &self,
            clipboard_contents_fn: impl FnOnce(&mut AppContext) -> ClipboardContent,
            ctx: &mut AppContext,
        ) {
            if self.linux_selection_clipboard_enabled() {
                let clipboard_content = clipboard_contents_fn(ctx);
                if !clipboard_content.plain_text.is_empty() {
                    ctx.clipboard()
                        .write_to_primary_clipboard(clipboard_content);
                }
            }
        }

        fn maybe_read_from_linux_selection_clipboard(
            &self,
            ctx: &mut AppContext,
        ) -> Option<ClipboardContent> {
            self.linux_selection_clipboard_enabled()
                .then(|| ctx.clipboard().read_from_primary_clipboard())
        }

        /// Implements the correct middle-click paste behavior for the current platform.
        ///
        /// Linux has the "primary clipboard" to which it maps the middle mouse button. Other platforms
        /// lack this separate clipboard, and so we map middle-click to the normal clipboard on those
        /// platforms.
        pub fn read_for_middle_click_paste(
            &self,
            ctx: &mut AppContext,
        ) -> Option<ClipboardContent> {
            if cfg!(any(target_os = "linux", target_os = "freebsd")) {
                return self.maybe_read_from_linux_selection_clipboard(ctx);
            }
            (self
                .middle_click_paste_enabled
                .is_supported_on_current_platform()
                && *self.middle_click_paste_enabled.value())
            .then(|| ctx.clipboard().read())
            .filter(|content| !content.is_empty())
        }
    }
}

/// Returns the clipboard's plain text, or its file paths joined by spaces when it holds files.
fn clipboard_text(mut content: ClipboardContent, replace_newlines_with_spaces: bool) -> String {
    if replace_newlines_with_spaces {
        content.plain_text = content.plain_text.replace('\n', " ");
    }
    match content.paths {
        Some(paths) => paths.join(" "),
        None => content.plain_text,
    }
}

trait RangeExt<T> {
    fn sorted(&self) -> (T, T);
}

impl<T: Ord + Clone> RangeExt<T> for Range<T> {
    fn sorted(&self) -> (T, T) {
        (
            cmp::min(&self.start, &self.end).clone(),
            cmp::max(&self.start, &self.end).clone(),
        )
    }
}

fn merge_ranges(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut i = 1;
    while i < ranges.len() {
        if ranges[i - 1].end.cmp(&ranges[i].start) >= Ordering::Equal {
            let removed = ranges.remove(i);
            if removed.start.cmp(&ranges[i - 1].start) < Ordering::Equal {
                ranges[i - 1].start = removed.start;
            }
            if removed.end.cmp(&ranges[i - 1].end) > Ordering::Equal {
                ranges[i - 1].end = removed.end;
            }
        } else {
            i += 1;
        }
    }
    ranges
}

/// Computes the grid cell size given the font and size at which the grid should
/// be rendered. We use a similar algorithm to Alacritty to do this, where the
/// cell width is the average advance of a glyph to its next (scaled up to pixel
/// coordinates instead of font coordinates) and the height is the line height.
fn grid_cell_dimensions(
    font_cache: &FontCache,
    font_family: FamilyId,
    font_size: f32,
    line_height_ratio: f32,
) -> Vector2F {
    let default_font_id = font_cache.select_font(font_family, Default::default());
    // Use the `m` character as a proxy for the average advance between two
    // characters in the font. It's safe to unwrap here as we expect the `m`
    // char to be present when we load the font.
    let (m_glyph, _) = font_cache
        .glyph_for_char(default_font_id, 'm', false)
        .expect("font must contain a glyph for the 'm' character");
    let advance = font_cache.glyph_advance(default_font_id, font_size, m_glyph);

    // Get the font metrics that make up the total line height for the font.
    let ascent = font_cache.ascent(default_font_id, font_size);
    let descent = font_cache.descent(default_font_id, font_size);
    let leading = font_cache.leading(default_font_id, font_size);
    // Using a ratio, compared to the default line height, to appropriately scale the cell's Height.
    let height = ((ascent - descent + leading)
        * (line_height_ratio / DEFAULT_UI_LINE_HEIGHT_RATIO))
        .ceil()
        .max(1.);

    match advance {
        Ok(advance) => {
            // The horizontal advance may be zero if the font is only meant to be rendered
            // vertically. In that case, default to an advance of 1.
            let horizontal_advance = if advance.x().is_zero() {
                1.
            } else {
                advance.x().round().max(1.)
            };
            Vector2F::new(horizontal_advance, height)
        }
        Err(error) => {
            // Return a default advance width if we couldn't load the advance from the glyph.
            report_error!(
                error.context("could not obtain advance for m glyph computing cell dimensions")
            );
            Vector2F::new(1.0, height)
        }
    }
}

/// Calculates the baseline position using Grid-style calculations, specifically using font metrics
/// such as leading/descent properties for the given font ID and font size.
fn calculate_grid_baseline_position(
    font_cache: &FontCache,
    font_family: FamilyId,
    font_size: f32,
    line_height_ratio: f32,
    cell_size_y: f32,
) -> Vector2F {
    let default_font_id = font_cache.select_font(font_family, Default::default());
    // Leading is additional spacing between lines.
    // Descent is the general guideline for how far the font will go below the baseline (for single spacing).
    let leading = font_cache.leading(default_font_id, font_size).floor();
    let descent = font_cache.descent(default_font_id, font_size).floor();
    // Adjust descent to be scaled up by the line height ratio to move up the baseline
    // and approximately maintain the same position within a line.
    vec2f(
        0.,
        cell_size_y - leading
            + (descent * (line_height_ratio / DEFAULT_UI_LINE_HEIGHT_RATIO).min(1.0)),
    )
}

/// Returns a closure that computes the baseline position in a Grid-style computation for the given font family.
fn grid_compute_baseline_position_fn(font_family: FamilyId) -> ComputeBaselinePositionFn {
    Box::new(move |font_metrics| {
        let cell_size_y = grid_cell_dimensions(
            font_metrics.font_cache,
            font_family,
            font_metrics.font_size,
            font_metrics.line_height_ratio,
        )
        .y();
        calculate_grid_baseline_position(
            font_metrics.font_cache,
            font_family,
            font_metrics.font_size,
            font_metrics.line_height_ratio,
            cell_size_y,
        )
        .y()
    })
}

trait SliceExt<T: 'static> {
    fn find_insertion_index<'a, F, E>(&'a self, compare: F) -> Result<usize, E>
    where
        F: FnMut(&'a T) -> Result<Ordering, E>;
}

impl<T: 'static> SliceExt<T> for [T] {
    fn find_insertion_index<'a, F, E>(&'a self, mut f: F) -> Result<usize, E>
    where
        F: FnMut(&'a T) -> Result<Ordering, E>,
    {
        use Ordering::*;

        let mut size = self.len();
        if size == 0 {
            return Ok(0);
        }
        let mut base = 0usize;
        while size > 1 {
            let half = size / 2;
            let mid = base + half;
            // mid is always in [0, size), that means mid is >= 0 and < size.
            // mid >= 0: by definition
            // mid < size: mid = size / 2 + size / 4 + size / 8 ...
            let cmp = f(&self[mid])?;
            base = if cmp == Greater { base } else { mid };
            size -= half;
        }
        let cmp = f(&self[base])?;
        if cmp == Equal {
            Ok(base)
        } else {
            Ok(base + (cmp == Less) as usize)
        }
    }
}

mod element {
    use std::ops::Range;
    use std::sync::Arc;
    use std::{cmp, mem};

    use pathfinder_geometry::rect::RectF;
    use pathfinder_geometry::vector::{Vector2F, vec2f};
    use smallvec::SmallVec;
    use warp_core::ui::appearance::{Appearance, DEFAULT_UI_FONT_SIZE};
    use warp_core::ui::theme::Fill;
    use warp_errors::report_error;
    use warp_util::user_input::UserInput;
    use warpui::elements::{
        AfterLayoutContext, CornerRadius, DEFAULT_UI_LINE_HEIGHT_RATIO, Element, Event,
        EventContext, LayoutContext, PaintContext, Point, Radius, SizeConstraint,
    };
    use warpui::event::{DispatchedEvent, ModifiersState};
    use warpui::text_layout::{
        self, ComputeBaselinePositionArgs, DEFAULT_TOP_BOTTOM_RATIO, LayoutCache,
    };
    use warpui::text_selection_utils::{
        NewlineTickParams, calculate_tick_width, create_newline_tick_rect,
        selection_crosses_newline_row_based,
    };
    use warpui::{AppContext, SingletonEntity};

    use super::model::MarkedTextState;
    use super::snapshot::ViewSnapshot;
    use super::soft_wrap::{
        ClampDirection, DisplayPointAndClampDirection, FrameLayouts, SoftWrapPoint, SoftWrapState,
    };
    use super::{
        CursorDisplayType, DisplayPoint, DrawableSelection, EditorAction,
        LocalDrawableSelectionData, ScrollState, SelectAction, position_id_for_cached_point,
        position_id_for_cursor, position_id_for_first_cursor,
    };

    // Similar to the terminal::model::ansi::CursorShape, this Editor Element has different cursor
    // shapes. However, this element doesn't implement all the same variants, so we don't share that
    // enum.
    /// Width for cursor analogous to CursorShape::Beam.
    const BEAM_CURSOR_WIDTH_PX: f32 = 3.;
    /// Width for cursor analogous to CursorShape::Block.
    const DEFAULT_BLOCK_CURSOR_WIDTH_PX: f32 = 8.;

    // Defined separately from LayoutState since LayoutState in used in broader
    // methods such as paint_lines where we use the final computed line_height.
    // In the future, we may add properties such as font kerning to this struct.
    pub struct LineParameters {
        line_height: f32,
        cursor_height: f32,
    }

    /// Colors to use when rendering text in the editor.
    #[derive(Clone, Debug)]
    pub struct TextColors {
        /// The color to use for regular text.
        pub default_color: Fill,
        /// The color to use when the editor is disabled.
        pub disabled_color: Fill,
        /// The color to use for suggestions/hints.
        pub hint_color: Fill,
    }

    impl TextColors {
        /// Default text colors based on the theme.
        pub fn from_appearance(appearance: &Appearance) -> Self {
            let theme = appearance.theme();
            Self {
                default_color: theme.main_text_color(theme.background()),
                disabled_color: theme.disabled_text_color(theme.background()),
                hint_color: theme.hint_text_color(theme.background()),
            }
        }

        pub fn all_hint_color(appearance: &Appearance) -> Self {
            let theme = appearance.theme();
            let hint_color = theme.hint_text_color(theme.background());
            Self {
                default_color: hint_color,
                disabled_color: hint_color,
                hint_color,
            }
        }
    }

    #[derive(Default)]
    pub struct EditorDecoratorElements {
        /// Arbitrary element that renders above the editor element. Currently used
        /// for the top n-1 lines of the lprompt.
        pub top_section: Option<Box<dyn Element>>,
        /// The left notch is an arbitrary element that is rendering at the
        /// top-left of the editor element. Note that we require this to be exactly
        /// 1 line high. The current use case for this is same-line prompt (this
        /// notch contains the nth, i.e. last, line of the lprompt).
        /// We start text painting right after this notch!
        pub left_notch: Option<Box<dyn Element>>,
        // Similarly, the right notch is currently used for the rprompt in the top-right,
        // but it could be any arbitrary element.
        pub right_notch: Option<Box<dyn Element>>,
        // The offset (in px) the right notch should be drawn, from the top-left of
        // the EditorElement (which is the top of the top section, if that exists).
        // If not specified, the default behavior is to draw the right notch at the right edge.
        pub right_notch_offset_px: Option<Vector2F>,
    }

    /// Structure holds the data necessary to draw cursors.
    /// The origin and width of the cursor vary based on its position in the buffer.
    struct CursorData {
        origin: Vector2F,
        /// The block cursor should take on the exact width of the glyph it's on, so it may vary
        /// per-cursor.
        block_cursor_width: f32,
        color: Fill,
    }

    /// Element that represents a text editor.
    pub struct EditorElement {
        view_snapshot: ViewSnapshot,
        scroll_state: ScrollState,
        layout: Option<LayoutState>,
        paint: Option<PaintState>,
        cursor_type: CursorDisplayType,
        soft_wrap: bool,
        /// Whether the placeholder text should soft wrap
        placeholder_soft_wrap: bool,
        /// Even when soft wrapping is off, we still use information about the
        /// laid out text in our code (the same logic just happens to work with
        /// or without soft wrapping).
        soft_wrap_state: SoftWrapState,
        text_colors: TextColors,
        editor_decorator_elements: EditorDecoratorElements,
        local_selection_data: LocalDrawableSelectionData,
    }

    impl EditorElement {
        /// Creates a new editor element.
        #[allow(clippy::too_many_arguments)]
        pub(super) fn new(
            view_snapshot: ViewSnapshot,
            scroll_state: ScrollState,
            soft_wrap: bool,
            soft_wrap_state: SoftWrapState,
            placeholder_soft_wrap: bool,
            text_colors: TextColors,
            editor_decorator_elements: EditorDecoratorElements,
            local_selection_data: LocalDrawableSelectionData,
            cursor_display_type: Option<CursorDisplayType>,
        ) -> Self {
            Self {
                view_snapshot,
                scroll_state,
                layout: None,
                paint: None,
                soft_wrap,
                placeholder_soft_wrap,
                soft_wrap_state,
                text_colors,
                editor_decorator_elements,
                local_selection_data,
                cursor_type: cursor_display_type.unwrap_or_default(),
            }
        }

        fn scroll_position(&self) -> Vector2F {
            *self.scroll_state.scroll_position.lock()
        }

        /// Handles selection actions.
        fn mouse_down(
            &self,
            position: Vector2F,
            modifiers: &ModifiersState,
            is_first_mouse: bool,
            ctx: &mut EventContext,
            app: &AppContext,
        ) -> bool {
            if !self.view_snapshot.can_select {
                return false;
            }
            let layout = self.layout.as_ref().unwrap();
            let paint = self.paint.as_ref().unwrap();
            if paint.rect.contains_point(position) {
                ctx.dispatch_typed_action(EditorAction::Focus);
                ctx.dispatch_typed_action(EditorAction::ClearParentSelections);

                if is_first_mouse {
                    // If the editor is receiving the first mouse click on activation
                    // we want to focus the editor but avoid starting any selections.
                    // No mouse up event is sent for the first mouse click, so we would
                    // otherwise get stuck in a selecting state.
                    return true;
                }
                let point_for_position = paint.possible_point_for_position(
                    &self.view_snapshot,
                    self.scroll_position(),
                    layout,
                    position,
                );
                if modifiers.shift {
                    ctx.dispatch_typed_action(EditorAction::Select(SelectAction::Extend {
                        position: point_for_position.display_point,
                        scroll_position: paint.scroll_position(
                            &self.view_snapshot,
                            &self.scroll_state,
                            layout,
                            position,
                            ctx,
                            app,
                        ),
                    }));
                } else {
                    let position = DisplayPointAndClampDirection {
                        point: point_for_position.display_point,
                        clamp_direction: point_for_position.clamp_direction,
                    };
                    ctx.dispatch_typed_action(EditorAction::Select(SelectAction::Begin {
                        position,
                        add: if cfg!(target_os = "macos") {
                            modifiers.cmd
                        } else {
                            modifiers.alt
                        },
                    }));
                }
                true
            } else {
                false
            }
        }

        /// Handles selection actions.
        fn double_mouse_down(&self, position: Vector2F, ctx: &mut EventContext) -> bool {
            if !self.view_snapshot.can_select {
                return false;
            }
            let layout = self.layout.as_ref().unwrap();
            let paint = self.paint.as_ref().unwrap();
            if paint.rect.contains_point(position) {
                let position = paint.point_for_position(
                    &self.view_snapshot,
                    self.scroll_position(),
                    layout,
                    position,
                );
                ctx.dispatch_typed_action(EditorAction::SelectWord(position));
                true
            } else {
                false
            }
        }

        /// Handles selection actions.
        fn triple_mouse_down(&self, position: Vector2F, ctx: &mut EventContext) -> bool {
            if !self.view_snapshot.can_select {
                return false;
            }
            let layout = self.layout.as_ref().unwrap();
            let paint = self.paint.as_ref().unwrap();
            if paint.rect.contains_point(position) {
                let position = paint.point_for_position(
                    &self.view_snapshot,
                    self.scroll_position(),
                    layout,
                    position,
                );
                ctx.dispatch_typed_action(EditorAction::SelectLine(position));
                true
            } else {
                false
            }
        }

        fn mouse_up(&self, _position: Vector2F, ctx: &mut EventContext, app: &AppContext) -> bool {
            if self.view_snapshot.is_selecting(app) {
                ctx.dispatch_typed_action(EditorAction::Select(SelectAction::End));
                true
            } else {
                false
            }
        }

        /// Handles selection actions.
        fn mouse_dragged(
            &self,
            position: Vector2F,
            ctx: &mut EventContext,
            app: &AppContext,
        ) -> bool {
            let layout = self.layout.as_ref().unwrap();
            let paint = self.paint.as_ref().unwrap();

            if self.view_snapshot.is_selecting(app) {
                ctx.dispatch_typed_action(EditorAction::Select(SelectAction::Update {
                    position: paint.point_for_position(
                        &self.view_snapshot,
                        self.scroll_position(),
                        layout,
                        position,
                    ),
                    scroll_position: paint.scroll_position(
                        &self.view_snapshot,
                        &self.scroll_state,
                        layout,
                        position,
                        ctx,
                        app,
                    ),
                }));
                true
            } else {
                false
            }
        }

        fn middle_mouse_down(&self, position: Vector2F, ctx: &mut EventContext) -> bool {
            if self
                .paint
                .as_ref()
                .is_some_and(|paint| paint.rect.contains_point(position))
            {
                ctx.dispatch_typed_action(EditorAction::MiddleClickPaste);
                return true;
            }
            false
        }

        fn typed_characters(&self, chars: &str, ctx: &mut EventContext) -> bool {
            if self.view_snapshot.is_focused {
                ctx.dispatch_typed_action(EditorAction::UserInsert(UserInput::new(chars)));
                return true;
            }
            false
        }

        fn set_marked_text(
            &mut self,
            marked_text: &str,
            selected_range: &Range<usize>,
            ctx: &mut EventContext,
        ) -> bool {
            if self.view_snapshot.is_focused {
                ctx.dispatch_typed_action(EditorAction::SetMarkedText {
                    marked_text: UserInput::new(marked_text),
                    selected_range: selected_range.clone(),
                });
                return true;
            }
            false
        }

        fn clear_marked_text(&mut self, ctx: &mut EventContext) -> bool {
            if self.view_snapshot.is_focused {
                ctx.dispatch_typed_action(EditorAction::ClearMarkedText);
                return true;
            }
            false
        }

        /// Handles mouse wheel and trackpad events
        fn scroll(
            &self,
            position: Vector2F,
            delta: Vector2F,
            precise: bool,
            ctx: &mut EventContext,
            app: &AppContext,
        ) -> bool {
            let paint = self.paint.as_ref().unwrap();

            if !paint.rect.contains_point(position) {
                return false;
            }

            let view_snapshot = &self.view_snapshot;
            let layout_cache = &ctx.text_layout_cache;

            let y;
            let x;
            if precise {
                let max_glyph_width = view_snapshot.em_width;
                let line_height = view_snapshot.line_height;
                x = (self.scroll_position().x() * max_glyph_width - delta.x()) / max_glyph_width;
                y = (self.scroll_position().y() * line_height - delta.y()) / line_height;
            } else {
                x = self.scroll_position().x() - delta.x();
                y = self.scroll_position().y() - delta.y()
            }

            let scroll_position = vec2f(x, y).clamp(
                Vector2F::zero(),
                self.layout
                    .as_ref()
                    .expect("should have layout")
                    .scroll_max(view_snapshot, layout_cache, app),
            );

            // Don't handle this event if this would be a noop so that a parent element
            // (such as the WaterFallGapElement) can properly handle scrolling
            if scroll_position != self.scroll_position() {
                ctx.dispatch_typed_action(EditorAction::Scroll(scroll_position));
                true
            } else {
                false
            }
        }

        fn visible_selection_range(
            selection: &mut Range<SoftWrapPoint>,
            visible_start: SoftWrapPoint,
            visible_end: SoftWrapPoint,
        ) -> Range<u32> {
            if selection.start > selection.end {
                mem::swap(&mut selection.start, &mut selection.end)
            }
            let visible_selection_start_row = cmp::max(selection.start.row(), visible_start.row());
            let visible_selection_end_row = cmp::min(selection.end.row(), visible_end.row());
            // If selection.end evaluates to visible_end (column 0), its row is not visible.
            // So we can skip painting it.
            if visible_selection_end_row == visible_end.row() && selection.end.column() == 0 {
                visible_selection_start_row..visible_selection_end_row
            } else {
                visible_selection_start_row..visible_selection_end_row + 1
            }
        }

        #[allow(clippy::too_many_arguments)]
        fn draw_selection(
            &self,
            content_origin: Vector2F,
            row: u32,
            first_visible_row: u32,
            color: Fill,
            selection: &Range<SoftWrapPoint>,
            line_height: f32,
            line_layout: &text_layout::Line,
            layout: &LayoutState,
            ctx: &mut PaintContext,
        ) {
            // Text content origin defines where we actually began painting the text, as opposed to content
            // origin which is the origin of the entire EditorElement. We push over the text content origin
            // if we're looking at the 0th row by the width of the left notch!
            let text_content_origin = if row == 0 {
                content_origin + vec2f(layout.left_notch_layout_width_px, 0.)
            } else {
                content_origin
            };

            let start_x = if row == selection.start.row() {
                line_layout.x_for_index(selection.start.column() as usize)
            } else {
                0.
            };
            let end_x = if row == selection.end.row() {
                line_layout.x_for_index(selection.end.column() as usize)
            } else {
                line_layout.width
            };

            ctx.scene
                .draw_rect_with_hit_recording(RectF::new(
                    text_content_origin
                        + vec2f(start_x, (row - first_visible_row) as f32 * line_height),
                    vec2f(end_x - start_x, line_height),
                ))
                .with_background(color);

            let max_soft_wrap_row = layout.frame_layouts.num_lines().saturating_sub(1) as u32;
            let is_last_line = row == max_soft_wrap_row;
            let selection_crosses_newline = selection_crosses_newline_row_based(
                row as usize,
                is_last_line,
                selection.start.row() as usize,
                selection.end.row() as usize,
                selection.end.column() as usize,
                line_layout.end_index(),
            );
            if selection_crosses_newline {
                let tick_width = calculate_tick_width(line_layout.font_size);
                let tick_origin = text_content_origin
                    + vec2f(
                        line_layout.width,
                        (row - first_visible_row) as f32 * line_height,
                    );
                ctx.scene
                    .draw_rect_with_hit_recording(create_newline_tick_rect(NewlineTickParams {
                        tick_origin,
                        tick_width,
                        tick_height: line_height,
                    }))
                    .with_background(color);
            }
        }

        /// Returns the height that the cursor should be.
        /// Note that the cursor's origin is NOT the top of the line - it is the "correct"
        /// spot at the top of the text. This means the cursor's height should be no larger than
        /// it would be at DEFAULT_UI_LINE_HEIGHT_RATIO.
        /// We do need to account for smaller line heights to prevent the cursor from extending too far down.
        pub fn cursor_height(font_size: f32, line_height_ratio: f32) -> f32 {
            font_size * DEFAULT_UI_LINE_HEIGHT_RATIO.min(line_height_ratio)
        }

        fn draw_cursors(
            cursor_display_type: CursorDisplayType,
            cursors: SmallVec<[CursorData; 32]>,
            view_snapshot: &ViewSnapshot,
            ctx: &mut PaintContext,
        ) {
            let cursor_height =
                Self::cursor_height(view_snapshot.font_size, view_snapshot.line_height_ratio);
            let cursor_corner_radius = match cursor_display_type {
                CursorDisplayType::Block | CursorDisplayType::Underline => Radius::Pixels(0.),
                CursorDisplayType::Bar => Radius::Percentage(50.),
            };
            for cursor in cursors {
                let cursor_width = match cursor_display_type {
                    CursorDisplayType::Block | CursorDisplayType::Underline => {
                        cursor.block_cursor_width
                    }
                    CursorDisplayType::Bar => BEAM_CURSOR_WIDTH_PX,
                };
                let mut cursor_rect = RectF::new(cursor.origin, vec2f(cursor_width, cursor_height));

                if cursor_display_type == CursorDisplayType::Underline {
                    cursor_rect.set_origin_y(cursor_rect.origin_y() + view_snapshot.font_size);
                };

                ctx.scene
                    .draw_rect_with_hit_recording(cursor_rect)
                    .with_background(cursor.color)
                    .with_corner_radius(CornerRadius::with_all(cursor_corner_radius));
            }
        }

        /// This is a helper method responsible for drawing lines within the editor (split into
        /// multiple selections). It returns vector of cursor positions (1 for every selection).
        fn draw_selections_and_get_cursor_data(
            &self,
            content_origin: Vector2F,
            bounds: RectF,
            layout: &LayoutState,
            line_parameters: LineParameters,
            ctx: &mut PaintContext,
            app: &AppContext,
        ) -> SmallVec<[CursorData; 32]> {
            let appearance = Appearance::as_ref(app);
            let fallback_block_cursor_width = DEFAULT_BLOCK_CURSOR_WIDTH_PX
                * (appearance.monospace_font_size() / DEFAULT_UI_FONT_SIZE);
            let view_snapshot = &self.view_snapshot;
            let marked_text_state = view_snapshot
                .editor_model
                .as_ref(app)
                .buffer(app)
                .marked_text_state();
            let first_visible_row = Self::first_visible_row(
                self.scroll_position().y(),
                view_snapshot.line_height,
                layout.top_section_height_px,
            );

            let scroll_top = self.scroll_position().y() * line_parameters.line_height;
            let visible_end_row =
                ((scroll_top + bounds.height()) / line_parameters.line_height).ceil() as u32;

            let mut cursors = SmallVec::<[CursorData; 32]>::new();

            // Looping over all selections.
            for (
                i,
                DrawableSelection {
                    range,
                    clamp_direction,
                },
            ) in view_snapshot
                .all_drawable_selections_intersecting_range(
                    DisplayPoint::new(0, 0)..view_snapshot.max_point(app),
                    app,
                )
                .enumerate()
            {
                let start = layout
                    .frame_layouts
                    .to_soft_wrap_point(range.start, clamp_direction);
                let end = layout
                    .frame_layouts
                    .to_soft_wrap_point(range.end, clamp_direction);

                let LocalDrawableSelectionData {
                    should_draw_cursors,
                    colors,
                } = self.local_selection_data;

                // Only attempt to draw the selection if both the start and end points exist.
                // There shouldn't be a case where we don't, but it's better to be safe.
                if let (Some(start), Some(end)) = (start, end) {
                    let mut selection = start..end;
                    let cursor_position = selection.end;
                    let text_content_origin = if cursor_position.row() == 0 {
                        content_origin + vec2f(layout.left_notch_layout_width_px, 0.)
                    } else {
                        content_origin
                    };

                    if (first_visible_row..visible_end_row).contains(&cursor_position.row()) {
                        let index = selection.end.row() as usize;
                        let cursor_row_layout = match layout.frame_layouts.get_line(index) {
                            Some(layout) => layout,
                            None => {
                                log::warn!(
                                    "Attempting to access line {index}, but there are fewer lines in the layout."
                                );
                                continue;
                            }
                        };
                        let cursor_x_index = match &marked_text_state {
                            MarkedTextState::Active { selected_range } => {
                                // The marked text selected range assumes that the marked text starts at 0.
                                // Adjust the cursor position to match what the IME tells us.
                                let ime_offset = selected_range.end;
                                selection.start.column() as usize + ime_offset
                            }
                            MarkedTextState::Inactive => selection.end.column() as usize,
                        };
                        // Use baseline position to get to bottom of text line, then subtract the font size to
                        // get to top of text. We have the multipliers of default line height ratio and top bottom ratio
                        // to get to the "correct" spot above the normal characters within a font.
                        // Note that we don't want to start from top of line (don't want
                        // a massive cursor for large line heights).
                        let cursor_origin = text_content_origin
                            + vec2f(
                                0.,
                                // 1. Go down by baseline position (which is 80% of line height due to top bottom ratio).
                                // Note that we do NOT have a particular Run (ie font ID) here, so we ALWAYS use the default Line-style
                                // baseline offset calculation for text selections.
                                (view_snapshot.baseline_position_fn()(
                                ComputeBaselinePositionArgs {
                                    font_cache: ctx.font_cache,
                                    font_size: cursor_row_layout.font_size,
                                    line_height_ratio: cursor_row_layout.line_height_ratio,
                                    baseline_ratio: cursor_row_layout.baseline_ratio,
                                    ascent: cursor_row_layout.ascent,
                                    descent: cursor_row_layout.descent,
                                },
                            )
                            // 2. Go up by font size * 80% (top bottom ratio) * 1.2 (default line height ratio) which
                            // gets us to a good point for the cursor origin (the same exact point in default case).
                                - (view_snapshot.font_size
                                    * DEFAULT_UI_LINE_HEIGHT_RATIO
                                    * DEFAULT_TOP_BOTTOM_RATIO))
                                    .max(0.0),
                            )
                            + vec2f(
                                cursor_row_layout.x_for_index(cursor_x_index),
                                (selection.end.row() - first_visible_row) as f32
                                    * line_parameters.line_height,
                            );

                        let block_cursor_width = cursor_row_layout
                            .width_for_index(selection.end.column() as usize)
                            .filter(|value| *value > 0.)
                            .unwrap_or(fallback_block_cursor_width);

                        if should_draw_cursors {
                            cursors.push(CursorData {
                                origin: cursor_origin,
                                block_cursor_width,
                                color: colors.cursor,
                            });
                        }
                        let cursor_size = vec2f(block_cursor_width, line_parameters.cursor_height);
                        ctx.position_cache.cache_position_indefinitely(
                            position_id_for_cursor(view_snapshot.view_id),
                            RectF::new(cursor_origin, cursor_size),
                        );
                        if i == 0 {
                            ctx.position_cache.cache_position_indefinitely(
                                position_id_for_first_cursor(view_snapshot.view_id),
                                RectF::new(cursor_origin, cursor_size),
                            );
                        }
                    }

                    let visible_start = SoftWrapPoint::new(first_visible_row, 0);
                    let visible_end = SoftWrapPoint::new(visible_end_row, 0);
                    if selection.start != selection.end {
                        let visible_selection_range = Self::visible_selection_range(
                            &mut selection,
                            visible_start,
                            visible_end,
                        );

                        for row in visible_selection_range {
                            let Some(line_layout) = layout.frame_layouts.get_line(row as usize)
                            else {
                                continue;
                            };

                            let selection_to_draw = match &marked_text_state {
                                MarkedTextState::Active { selected_range }
                                    if !selected_range.is_empty() =>
                                {
                                    // This is the case where we have a selected range within marked text.
                                    // This selected range needs to be highlighted.
                                    // Since we model the marked text itself as a selection in the editor,
                                    // we need to generate the "real" selection here.
                                    let marked_text_selection_start = SoftWrapPoint::new(
                                        selection.start.row(),
                                        selection.start.column() + selected_range.start as u32,
                                    );
                                    let marked_text_selection_end = SoftWrapPoint::new(
                                        selection.start.row(),
                                        selection.start.column() + selected_range.end as u32,
                                    );
                                    let marked_text_selection =
                                        marked_text_selection_start..marked_text_selection_end;
                                    Some(marked_text_selection)
                                }
                                MarkedTextState::Inactive => Some(selection.clone()),
                                _ => None,
                            };

                            if let Some(selection_to_draw) = selection_to_draw {
                                self.draw_selection(
                                    content_origin,
                                    row,
                                    first_visible_row,
                                    colors.selection,
                                    &selection_to_draw,
                                    line_parameters.line_height,
                                    line_layout,
                                    layout,
                                    ctx,
                                );
                            }
                        }
                    }
                }
            }

            cursors
        }

        /// Returns the index of the first visible row, given the current scroll position, taking into
        /// account the top section element, if relevant.
        fn first_visible_row(
            scroll_position_y: f32,
            line_height: f32,
            top_section_height_px: f32,
        ) -> u32 {
            let top_section_height_lines = top_section_height_px / line_height;
            if scroll_position_y <= top_section_height_lines {
                return 0;
            }
            // Calculate the scroll position relative to the start of the user typed editor content.
            let adjusted_scroll_position_y = scroll_position_y - top_section_height_lines;
            adjusted_scroll_position_y.floor() as u32
        }

        /// Returns the fraction of the scroll position (y) that is drawn "above" the visible content area.
        fn scroll_position_y_fract(
            scroll_position_y: f32,
            line_height: f32,
            top_section_height_px: f32,
        ) -> f32 {
            // We draw the top section until the first visible row is no longer 0. Hence, we need to account for the first
            // row, which is beside the left notch.
            if Self::first_visible_row(scroll_position_y, line_height, top_section_height_px) == 0 {
                return scroll_position_y * line_height;
            }

            let top_section_height_lines = top_section_height_px / line_height;
            let adjusted_scroll_position_y = scroll_position_y - top_section_height_lines;
            // Otherwise, we're simply drawing the first visible row, which can go slightly outside of the visible area.
            adjusted_scroll_position_y.fract() * line_height
        }

        #[allow(clippy::too_many_arguments)]
        fn paint_lines(
            &self,
            content_origin: Vector2F,
            layout: &LayoutState,
            line_height: f32,
            ctx: &mut PaintContext,
            last_placeholder_glyph_position: &mut Option<Vector2F>,
        ) {
            let view_snapshot = &self.view_snapshot;
            let first_visible_row = Self::first_visible_row(
                self.scroll_position().y(),
                line_height,
                layout.top_section_height_px,
            );
            let color = if view_snapshot.can_select {
                self.text_colors.default_color
            } else {
                self.text_colors.disabled_color
            }
            .into();

            let baseline_position_fn = view_snapshot.baseline_position_fn();

            for (ix, line) in layout.frame_layouts.displayed_lines().enumerate() {
                // Push over text content origin due to left notch, if the first line is visible.
                let text_content_origin = if ix == 0 && first_visible_row == 0 {
                    content_origin + vec2f(layout.left_notch_layout_width_px, 0.)
                } else {
                    content_origin
                };
                let line_origin = text_content_origin + vec2f(0., ix as f32 * line_height);

                line.paint_with_baseline_position(
                    RectF::from_points(
                        line_origin,
                        self.bounds()
                            .expect("layout() should have been called before paint()")
                            .lower_right(),
                    ),
                    &Default::default(),
                    color,
                    ctx.font_cache,
                    ctx.scene,
                    &baseline_position_fn,
                );

                let is_last_line = ix == layout.frame_layouts.displayed_lines().count() - 1;
                if is_last_line
                    && let Some(placeholder_line) = layout.placeholder_text_line_layouts.first()
                {
                    let line_origin = line_origin + vec2f(line.width, 0.);

                    placeholder_line.paint_with_baseline_position(
                        RectF::from_points(
                            line_origin,
                            self.bounds()
                                .expect("layout() should have been called before paint()")
                                .lower_right(),
                        ),
                        &Default::default(),
                        self.text_colors.hint_color.into(),
                        ctx.font_cache,
                        ctx.scene,
                        &baseline_position_fn,
                    );
                    if let Some(pos) = last_placeholder_glyph_position {
                        *pos = line_origin + vec2f(placeholder_line.width, 0.);
                    }
                }
            }
        }

        /// Computes right notch origin, if it should be drawn. Otherwise, returns None.
        fn compute_right_notch_origin(
            &self,
            layout: &LayoutState,
            first_visible_row: u32,
            element_origin: Vector2F,
            content_origin: Vector2F,
            last_placeholder_glyph_position: Option<Vector2F>,
        ) -> Option<Vector2F> {
            if first_visible_row != 0 {
                return None;
            }

            // Use the right notch offset, to compute the right notch origin, if it exists,
            // otherwise, default to the right edge of the EditorElement.
            let right_notch_origin = self
                .editor_decorator_elements
                .right_notch_offset_px
                .map(|right_notch_offset_px| element_origin + right_notch_offset_px)
                .unwrap_or_else(|| {
                    vec2f(
                        layout.size.x() - layout.right_notch_layout_width_px + content_origin.x(),
                        content_origin.y(),
                    )
                });

            let first_line_intersects_right_notch = layout
                .frame_layouts
                .get_line(0)
                .map(|first_line| {
                    content_origin.x() + first_line.width + layout.left_notch_layout_width_px
                        > right_notch_origin.x()
                })
                .unwrap_or(false);

            let placeholder_intersects_right_notch = last_placeholder_glyph_position
                .map(|position| {
                    // The placeholder must be on the same line as the right notch, to intersect it.
                    position.y() == right_notch_origin.y() && position.x() > right_notch_origin.x()
                })
                .unwrap_or(false);

            // We only want to paint the right notch, if there is no intersecting first line of text or placeholder.
            (!first_line_intersects_right_notch && !placeholder_intersects_right_notch)
                .then_some(right_notch_origin)
        }

        /// Note that we lay out all of the rows instead of only the visible ones.
        fn layout_text_frames(
            &self,
            view_snapshot: &ViewSnapshot,
            size: &Vector2F,
            left_notch_layout_width_px: f32,
            top_section_height_px: f32,
            ctx: &LayoutContext,
            app: &AppContext,
        ) -> anyhow::Result<FrameLayouts> {
            let line_height = view_snapshot.line_height;

            let max_width = if self.soft_wrap { size.x() } else { f32::MAX };
            let frames = view_snapshot.layout_text_frames(
                0..view_snapshot.max_point(app).row() + 1,
                ctx.text_layout_cache,
                max_width,
                left_notch_layout_width_px,
                app,
            )?;

            // Determine a limit for the end line. If the size is infinite,
            // this is infinite. Otherwise, depending on the scrolling position
            // and the number of lines that fit in the element, calculate the line index.
            let end_line = if size.y().is_infinite() {
                u32::MAX
            } else {
                let scroll_top = self.scroll_position().y() * line_height + top_section_height_px;
                ((scroll_top + size.y()) / line_height).ceil() as u32
            };
            let start_line = Self::first_visible_row(
                self.scroll_position().y(),
                line_height,
                top_section_height_px,
            );
            Ok(FrameLayouts::new(frames, start_line, end_line))
        }
    }

    impl Element for EditorElement {
        fn layout(
            &mut self,
            constraint: SizeConstraint,
            ctx: &mut LayoutContext,
            app: &AppContext,
        ) -> Vector2F {
            let mut size = constraint.max;
            let view_snapshot = &self.view_snapshot;

            let font_cache = app.font_cache();
            let line_height = view_snapshot.line_height;

            let top_section_height_px = self
                .editor_decorator_elements
                .top_section
                .as_mut()
                .map(|top_section_element: &mut Box<dyn Element>| {
                    top_section_element.layout(constraint, ctx, app).y()
                })
                .unwrap_or(0.0);
            // We require the left notch to be exactly 1 line_height high! We do this
            // to avoid tracking y-offset logic in the EditorElement (for aligning to
            // the bottom of the left notch). Hence, we only track the width.
            let left_notch_layout_width_px = self
                .editor_decorator_elements
                .left_notch
                .as_mut()
                .map(|left_notch_element| left_notch_element.layout(constraint, ctx, app).x())
                .unwrap_or(0.0);
            let right_notch_layout_width_px = self
                .editor_decorator_elements
                .right_notch
                .as_mut()
                .map(|right_notch_element| right_notch_element.layout(constraint, ctx, app).x())
                .unwrap_or(0.0);

            let frame_layouts = match self.layout_text_frames(
                view_snapshot,
                &size,
                left_notch_layout_width_px,
                top_section_height_px,
                ctx,
                app,
            ) {
                Err(error) => {
                    report_error!(error.context("error laying out lines"));
                    return size;
                }
                Ok(layouts) => layouts,
            };

            let last_text_line_width = frame_layouts
                .last_frame()
                .and_then(|frame| frame.lines().last().map(|line| line.width))
                .unwrap_or(0.);

            let placeholder_text_line_layouts = view_snapshot
                .matching_placeholder_text(&view_snapshot.editor_model.as_ref(app).buffer_text(app))
                .map(|placeholder_text| {
                    view_snapshot.layout_placeholder_text(
                        &placeholder_text,
                        last_text_line_width + left_notch_layout_width_px,
                        font_cache,
                        ctx.text_layout_cache,
                        &size,
                        self.placeholder_soft_wrap,
                    )
                })
                .unwrap_or_default();
            // Set the height of the element, in case we don't have enough text to fill
            // in all the space.
            if size.y().is_infinite() || view_snapshot.autogrow {
                let num_lines_total =
                    if view_snapshot.is_empty && view_snapshot.placeholder_text_exists() {
                        placeholder_text_line_layouts.len() as u32
                    } else {
                        frame_layouts
                            .frames()
                            .fold(0, |acc, layout| acc + layout.lines().len() as u32)
                            .max(1)
                    };

                // We add 1. to the height because when we paint the text content, we force pixel-alignment of the Editor's content
                // to avoid rendering artifacts with text. This is done by taking the ceil of the content_origin.y().
                // This can increase the required height by up to 1, and we don't know if we're going to need to do this until paint time.
                let computed_y =
                    num_lines_total as f32 * view_snapshot.line_height + top_section_height_px + 1.;

                size.set_y(computed_y.clamp(constraint.min.y(), size.y()));
            }
            if size.x().is_infinite() {
                unimplemented!(
                    "we don't yet handle an infinite width constraint on buffer elements"
                );
            }

            let top_section_height_lines = top_section_height_px / view_snapshot.line_height;
            let total_lines = frame_layouts
                .frames()
                .fold(0, |acc, frame| acc + frame.lines().len())
                as f32
                + top_section_height_lines;

            let autoscroll_horizontally = view_snapshot.autoscroll_vertically(
                &self.scroll_state,
                total_lines,
                size.y() / line_height,
                top_section_height_lines,
                &frame_layouts,
                app,
            ) && !self.soft_wrap;

            let mut max_visible_line_width = frame_layouts
                .frames()
                .map(|x| x.max_width())
                .reduce(f32::max)
                .unwrap_or(0.);

            max_visible_line_width = max_visible_line_width.max(
                placeholder_text_line_layouts
                    .iter()
                    .map(|line| line.width)
                    .reduce(f32::max)
                    .unwrap_or(0.),
            );

            // The first placeholder line is rendered on the same line as the last line of the
            // buffer, so update the max width to include it.
            if let Some(placeholder_line) = placeholder_text_line_layouts.first() {
                max_visible_line_width =
                    max_visible_line_width.max(last_text_line_width + placeholder_line.width);
            }

            self.soft_wrap_state.update(frame_layouts.clone());

            self.layout = Some(LayoutState {
                size,
                frame_layouts,
                placeholder_text_line_layouts,
                max_visible_line_width,
                autoscroll_horizontally,
                top_section_height_px,
                left_notch_layout_width_px,
                right_notch_layout_width_px,
            });
            size
        }

        fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
            if let Some(layout) = &self.layout {
                if let Some(top_section) = &mut self.editor_decorator_elements.top_section {
                    top_section.after_layout(ctx, app);
                }

                if let Some(left_notch) = &mut self.editor_decorator_elements.left_notch {
                    left_notch.after_layout(ctx, app);
                }

                if let Some(right_notch) = &mut self.editor_decorator_elements.right_notch {
                    right_notch.after_layout(ctx, app);
                }

                let view_snapshot = &self.view_snapshot;
                view_snapshot.clamp_scroll_left(
                    &self.scroll_state,
                    layout
                        .scroll_max(view_snapshot, ctx.text_layout_cache, app)
                        .x(),
                );

                if layout.autoscroll_horizontally {
                    view_snapshot.autoscroll_horizontally(
                        &self.scroll_state,
                        self.scroll_position().y() as u32,
                        layout.size.x(),
                        layout.scroll_width(view_snapshot, ctx.text_layout_cache, app),
                        view_snapshot.em_width,
                        layout
                            .frame_layouts
                            .frames()
                            .flat_map(|frame| frame.lines().first())
                            .collect(),
                        app,
                    );
                }
            }
        }

        // TODO: refactor this paint code to make it more readable.
        fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
            if let Some(layout) = &self.layout {
                let bounds = RectF::new(origin, layout.size);
                self.paint = Some(PaintState {
                    rect: bounds,
                    origin: Point::from_vec2f(origin, ctx.scene.z_index()),
                });
                let view_snapshot = &self.view_snapshot;
                let cursor_height =
                    Self::cursor_height(view_snapshot.font_size, view_snapshot.line_height_ratio);
                let line_height = view_snapshot.line_height;
                // The start of the element (top-left of top section).
                let element_origin = bounds.origin()
                    - vec2f(
                        self.scroll_position().x() * view_snapshot.em_width,
                        Self::scroll_position_y_fract(
                            self.scroll_position().y(),
                            line_height,
                            layout.top_section_height_px,
                        ),
                    );

                let first_visible_row = Self::first_visible_row(
                    self.scroll_position().y(),
                    line_height,
                    layout.top_section_height_px,
                );

                // The start of the "core" content, below the top section.
                // Note that the Grid is always pixel-aligned to avoid rendering artifacts that cause text to look misaligned.
                // Similarly, we force pixel-alignment of the Editor's content to avoid rendering artifacts with text.
                let content_origin;
                if first_visible_row == 0 {
                    if let Some(top_section_element) =
                        &mut self.editor_decorator_elements.top_section
                    {
                        top_section_element.paint(element_origin, ctx, app);
                        content_origin = vec2f(
                            element_origin.x(),
                            element_origin.y() + layout.top_section_height_px,
                        )
                        .ceil();
                    } else {
                        content_origin = element_origin.ceil();
                    }

                    if let Some(left_notch_element) = &mut self.editor_decorator_elements.left_notch
                    {
                        left_notch_element.paint(content_origin, ctx, app);
                    }
                } else {
                    content_origin = element_origin.ceil();
                }

                let cursors = self.draw_selections_and_get_cursor_data(
                    content_origin,
                    bounds,
                    layout,
                    LineParameters {
                        line_height,
                        cursor_height,
                    },
                    ctx,
                    app,
                );

                Self::draw_cursors(self.cursor_type, cursors, view_snapshot, ctx);

                let mut last_placeholder_glyph_position = Some(vec2f(0., 0.));
                self.paint_lines(
                    content_origin,
                    layout,
                    line_height,
                    ctx,
                    &mut last_placeholder_glyph_position,
                );

                let right_notch_origin = self.compute_right_notch_origin(
                    layout,
                    first_visible_row,
                    element_origin,
                    content_origin,
                    last_placeholder_glyph_position,
                );

                right_notch_origin.and_then(|right_notch_origin| {
                    self.editor_decorator_elements
                        .right_notch
                        .as_mut()
                        .map(|right_notch_element| {
                            right_notch_element.paint(right_notch_origin, ctx, app);
                        })
                });

                let line_origin = content_origin
                    + vec2f(
                        0.,
                        layout.frame_layouts.num_displayed_lines() as f32 * line_height,
                    );

                // The first placeholder line is painted by paint_lines, on the last buffer line.
                layout
                    .placeholder_text_line_layouts
                    .iter()
                    .skip(1)
                    .enumerate()
                    .for_each(|(ix, line)| {
                        let line_origin = line_origin + vec2f(0., ix as f32 * line_height);
                        line.paint_with_baseline_position(
                            RectF::from_points(
                                line_origin,
                                self.bounds()
                                    .expect("layout() should have been called before paint()")
                                    .lower_right(),
                            ),
                            &Default::default(),
                            self.text_colors.hint_color.into(),
                            ctx.font_cache,
                            ctx.scene,
                            &view_snapshot.baseline_position_fn(),
                        );
                    });

                let cursor_size = vec2f(BEAM_CURSOR_WIDTH_PX, cursor_height);
                for (id, point) in &view_snapshot.cached_buffer_points {
                    // `cached_buffer_points` are expressed in buffer (DisplayPoint) coordinates, but
                    // the editor's line layouts are indexed by soft-wrapped row. Convert before
                    // looking up a line / computing pixel offsets.
                    let display_point = DisplayPoint::new(point.row, point.column);
                    let Some(soft_wrap_point) = layout
                        .frame_layouts
                        .to_soft_wrap_point(display_point, ClampDirection::Down)
                    else {
                        continue;
                    };

                    let row = soft_wrap_point.row();
                    let Some(line) = &layout.frame_layouts.get_line(row as usize) else {
                        continue;
                    };

                    // `content_origin` is the origin of the first visible row.
                    let relative_row = row as f32 - first_visible_row as f32;
                    let y_offset = relative_row * line_height;

                    // Match paint_lines: the left notch only affects the first visible line.
                    let text_content_origin = if row == 0 && first_visible_row == 0 {
                        content_origin + vec2f(layout.left_notch_layout_width_px, 0.)
                    } else {
                        content_origin
                    };

                    let x_offset = line.x_for_index(soft_wrap_point.column() as usize);
                    let bounds = text_content_origin + vec2f(x_offset, y_offset);

                    ctx.position_cache.cache_position_indefinitely(
                        position_id_for_cached_point(self.view_snapshot.view_id, id),
                        RectF::new(bounds, cursor_size),
                    );
                }
            }
        }

        fn dispatch_event(
            &mut self,
            event: &DispatchedEvent,
            ctx: &mut EventContext,
            app: &AppContext,
        ) -> bool {
            let Some(z_index) = self.z_index() else {
                return false;
            };

            // Since editor decorator elements may be clickable, we need to prioritize dispatching events from them.
            if let Some(top_section) = &mut self.editor_decorator_elements.top_section
                && top_section.dispatch_event(event, ctx, app)
            {
                return true;
            }

            if let Some(left_notch) = &mut self.editor_decorator_elements.left_notch
                && left_notch.dispatch_event(event, ctx, app)
            {
                return true;
            }

            if let Some(right_notch) = &mut self.editor_decorator_elements.right_notch
                && right_notch.dispatch_event(event, ctx, app)
            {
                return true;
            }

            let Some(event_at_z_index) = event.at_z_index(z_index, ctx) else {
                return false;
            };

            let events_to_propagate_on = matches!(
                event_at_z_index,
                Event::MouseMoved { .. }
                    | Event::LeftMouseDragged { .. }
                    | Event::LeftMouseDown { .. }
                    | Event::LeftMouseUp { .. }
                    | Event::RightMouseDown { .. }
            );

            // Intercept mouse events for floating elements,
            // similar to how it is done in dispatch_event() in block_list_element.rs
            if events_to_propagate_on {
                // Only pass through events to decorator elements which have been painted (this avoids cases such as the right prompt
                // not being painted, where the EventHandler will fail to unwrap a Z-index option)!
                if let Some(top_section) = self
                    .editor_decorator_elements
                    .top_section
                    .as_mut()
                    .filter(|n| n.z_index().is_some())
                    && top_section.dispatch_event(event, ctx, app)
                {
                    return true;
                }
                if let Some(left_notch) = self
                    .editor_decorator_elements
                    .left_notch
                    .as_mut()
                    .filter(|n| n.z_index().is_some())
                    && left_notch.dispatch_event(event, ctx, app)
                {
                    return true;
                }
                if let Some(right_notch) = self
                    .editor_decorator_elements
                    .right_notch
                    .as_mut()
                    .filter(|n| n.z_index().is_some())
                    && right_notch.dispatch_event(event, ctx, app)
                {
                    return true;
                }
            }

            match event_at_z_index {
                Event::LeftMouseDown {
                    position,
                    modifiers,
                    click_count,
                    is_first_mouse,
                } => match *click_count {
                    1 => self.mouse_down(*position, modifiers, *is_first_mouse, ctx, app),
                    2 => self.double_mouse_down(*position, ctx),
                    3 => self.triple_mouse_down(*position, ctx),
                    _ => false,
                },
                Event::LeftMouseUp { position, .. } => self.mouse_up(*position, ctx, app),
                Event::LeftMouseDragged { position, .. } => self.mouse_dragged(*position, ctx, app),
                Event::ScrollWheel {
                    position,
                    delta,
                    precise,
                    modifiers: ModifiersState { ctrl: false, .. },
                } => self.scroll(*position, *delta, *precise, ctx, app),
                Event::TypedCharacters { chars } => self.typed_characters(chars, ctx),
                Event::MiddleMouseDown { position, .. } => self.middle_mouse_down(*position, ctx),
                Event::SetMarkedText {
                    marked_text,
                    selected_range,
                } => self.set_marked_text(marked_text, selected_range, ctx),
                Event::ClearMarkedText => self.clear_marked_text(ctx),
                _ => false,
            }
        }

        fn size(&self) -> Option<Vector2F> {
            self.layout.as_ref().map(|layout| layout.size)
        }

        fn origin(&self) -> Option<Point> {
            self.paint.as_ref().map(|paint| paint.origin)
        }
    }

    struct LayoutState {
        size: Vector2F,
        // Each frame is a row of text from the buffer. If the soft wrap option is on and a row
        // is too long to fit on the screen, it'll be split into multiple lines within
        // a frame. Otherwise, the frame is a singleton Line.
        frame_layouts: FrameLayouts,
        /// The placeholder text, or empty if no placeholder is shown.
        placeholder_text_line_layouts: Vec<Arc<text_layout::Line>>,
        max_visible_line_width: f32,
        /// True if the `autoscroll_vertically` function on the editor view returns true
        /// and the soft wrap setting is off.
        autoscroll_horizontally: bool,
        /// Height of top section element, rendered above the EditorElement (defaults to 0
        /// if no top element).
        top_section_height_px: f32,
        /// The width of the left notch and right notch elements when laid out (default to 0 if
        /// they doesn't exist).
        left_notch_layout_width_px: f32,
        right_notch_layout_width_px: f32,
    }

    impl LayoutState {
        /// Computes the scroll width of this editor in pixels.
        fn scroll_width(
            &self,
            view_snapshot: &ViewSnapshot,
            text_layout_cache: &LayoutCache,
            app: &AppContext,
        ) -> f32 {
            if self.autoscroll_horizontally {
                let row = view_snapshot.rightmost_point(app).row();
                let longest_line_width = view_snapshot
                    .layout_line(row, text_layout_cache, app)
                    .expect("Should have layout line")
                    .width;
                longest_line_width.max(self.max_visible_line_width) + view_snapshot.em_width
            } else {
                let last_line_width = self
                    .frame_layouts
                    .displayed_lines()
                    .last()
                    .map_or(0., |line| line.width);
                let placeholder_width = self
                    .placeholder_text_line_layouts
                    .first()
                    .map_or(0., |line| line.width);
                last_line_width + placeholder_width + view_snapshot.em_width
            }
        }

        /// The maximum bottom-right scrolling position in pixels.
        fn scroll_max(
            &self,
            view_snapshot: &ViewSnapshot,
            text_layout_cache: &LayoutCache,
            app: &AppContext,
        ) -> Vector2F {
            let horizontal_scrolling_max =
                ((self.scroll_width(view_snapshot, text_layout_cache, app) - self.size.x())
                    / view_snapshot.em_width)
                    .max(0.0);

            // Total number of lines, including the top section.
            let total_lines =
                self.frame_layouts
                    .frames()
                    .fold(0, |acc, frame| acc + frame.lines().len()) as f32
                    + self.top_section_height_px / view_snapshot.line_height;

            let max_scroll_top = ViewSnapshot::max_scroll_top(
                total_lines,
                self.size.y() / view_snapshot.line_height,
            );
            vec2f(horizontal_scrolling_max, max_scroll_top)
        }
    }

    struct PaintState {
        rect: RectF,
        origin: Point,
    }

    /// The display point might be clamped at either end of a line.
    pub struct PossibleDisplayPoint {
        pub display_point: DisplayPoint,
        pub clamp_direction: ClampDirection,
    }

    impl PaintState {
        /// Returns the display point for the given position,
        /// clamping the row/col if necessary.
        fn point_for_position(
            &self,
            view_snapshot: &ViewSnapshot,
            scroll_position: Vector2F,
            layout: &LayoutState,
            position: Vector2F,
        ) -> DisplayPoint {
            self.possible_point_for_position(view_snapshot, scroll_position, layout, position)
                .display_point
        }

        /// Returns a possible display point for the given position,
        /// which includes whether or not the display_point has been clamped.
        ///
        /// This method supports soft-wrapping.
        fn possible_point_for_position(
            &self,
            view_snapshot: &ViewSnapshot,
            scroll_position: Vector2F,
            layout: &LayoutState,
            position: Vector2F,
        ) -> PossibleDisplayPoint {
            let position = position - self.rect.origin();
            let y = position.y().max(0.0).min(layout.size.y());

            let num_rows = layout.frame_layouts.num_lines();
            let max_row = num_rows as u32 - 1;
            // Offset by the top section, which shouldn't be considered for purposes of position -> display point.
            let shifted_scroll_position_y =
                scroll_position.y() - layout.top_section_height_px / view_snapshot.line_height;
            let mut row: u32 = ((y / view_snapshot.line_height) + shifted_scroll_position_y) as u32;
            if row > max_row {
                row = max_row;
            }
            let shifted_position = if row == 0 {
                // Note that the given position is shifted for the left notch's width, but we don't
                // want the DisplayPoint col to be shifted over, hence we subtract the width.
                vec2f(
                    position.x() - layout.left_notch_layout_width_px,
                    position.y(),
                )
            } else {
                position
            };

            let line = layout
                .frame_layouts
                .get_line(row as usize)
                .expect("Should be clamped to last possible line");

            let x = shifted_position.x() + (scroll_position.x() * view_snapshot.em_width);

            let col = if let Some(col) = line.index_for_x(x).map(|ix| ix as u32) {
                col
            } else {
                // Clamp to the left or right if the x pos is before or after the buffer text, respectively.
                if x >= 0. {
                    // If the line has no glyphs, the index is zero. Otherwise, we take one more index
                    // beyond the last start index in the line to get the last position.
                    line.last_glyph()
                        .map_or(0, |glyph| (glyph.index + 1) as u32)
                } else {
                    0
                }
            };
            // Now convert from SoftWrapPoint to DisplayPoint.
            let soft_wrap_point = SoftWrapPoint::new(row, col);

            let display_point_and_clamp_direction =
                layout.frame_layouts.to_display_point(soft_wrap_point);
            PossibleDisplayPoint {
                display_point: display_point_and_clamp_direction.point,
                clamp_direction: display_point_and_clamp_direction.clamp_direction,
            }
        }

        #[allow(clippy::too_many_arguments)]
        fn scroll_position(
            &self,
            view_snapshot: &ViewSnapshot,
            scroll_state: &ScrollState,
            layout: &LayoutState,
            position: Vector2F,
            ctx: &EventContext,
            app: &AppContext,
        ) -> Vector2F {
            let rect = self.rect;
            let mut scroll_delta = Vector2F::zero();

            let vertical_margin = view_snapshot.line_height.min(rect.height() / 3.0);
            let top = rect.origin_y() + vertical_margin;
            let bottom = rect.lower_left().y() - vertical_margin;
            if position.y() < top {
                scroll_delta.set_y(-scale_vertical_mouse_autoscroll_delta(top - position.y()))
            }
            if position.y() > bottom {
                scroll_delta.set_y(scale_vertical_mouse_autoscroll_delta(position.y() - bottom))
            }

            let horizontal_margin = view_snapshot.line_height.min(rect.width() / 3.0);
            let left = rect.origin_x() + horizontal_margin;
            let right = rect.upper_right().x() - horizontal_margin;
            if position.x() < left {
                scroll_delta.set_x(-scale_horizontal_mouse_autoscroll_delta(
                    left - position.x(),
                ))
            }
            if position.x() > right {
                scroll_delta.set_x(scale_horizontal_mouse_autoscroll_delta(
                    position.x() - right,
                ))
            }
            (scroll_state.scroll_position() + scroll_delta).clamp(
                Vector2F::zero(),
                layout.scroll_max(view_snapshot, ctx.text_layout_cache, app),
            )
        }
    }

    fn scale_vertical_mouse_autoscroll_delta(delta: f32) -> f32 {
        delta.powf(1.5) / 100.0
    }

    fn scale_horizontal_mouse_autoscroll_delta(delta: f32) -> f32 {
        delta.powf(1.2) / 300.0
    }
}

mod snapshot {
    use std::borrow::Cow;
    use std::cmp;
    use std::collections::HashMap;
    use std::ops::Range;
    use std::sync::Arc;

    use anyhow::Result;
    use pathfinder_geometry::vector::Vector2F;
    use rayon::prelude::*;
    use warp_errors::report_error;
    use warpui::fonts::{Cache as FontCache, FamilyId, Properties};
    use warpui::platform::LineStyle;
    use warpui::text::point::Point;
    use warpui::text_layout::{
        self, ClipConfig, ComputeBaselinePositionFn, DEFAULT_TOP_BOTTOM_RATIO, LayoutCache,
        StyleAndFont, TextAlignment, TextStyle, default_compute_baseline_position_fn,
    };
    use warpui::{AppContext, EntityId, ModelHandle};

    use super::model::EditorModel;
    use super::soft_wrap::FrameLayouts;
    use super::{
        BaselinePositionComputationMethod, DisplayPoint, DrawableSelection, ScrollState,
        ToDisplayPoint, grid_compute_baseline_position_fn,
    };

    /// A read-only snapshot of the [`EditorView`] that is needed
    /// to render the [`EditorElement`].
    pub struct ViewSnapshot {
        pub view_id: EntityId,
        pub is_focused: bool,
        pub editor_model: ModelHandle<EditorModel>,

        pub can_select: bool,

        pub font_size: f32,
        pub font_family: FamilyId,
        pub placeholder_font_family: FamilyId,
        pub font_properties: Properties,
        pub line_height: f32,
        pub line_height_ratio: f32,
        pub em_width: f32,

        pub autogrow: bool,
        pub is_empty: bool,

        /// Map from prefix to placeholder text. The view can have multiple active prefixes that when
        /// exactly matched in the buffer cause additional placeholder ghost text to be displayed. The
        /// empty string prefix "" is the default placeholder (shown when buffer is empty).
        pub placeholder_texts: Arc<HashMap<String, String>>,

        pub cached_buffer_points: HashMap<Cow<'static, str>, Point>,

        pub baseline_position_computation_method: BaselinePositionComputationMethod,
    }

    impl ViewSnapshot {
        pub fn baseline_position_fn(&self) -> ComputeBaselinePositionFn {
            // Copy the font family ID so that it can be moved into the closure.
            let font_family = self.font_family;
            match self.baseline_position_computation_method {
                BaselinePositionComputationMethod::Grid => {
                    grid_compute_baseline_position_fn(font_family)
                }
                BaselinePositionComputationMethod::Default => {
                    default_compute_baseline_position_fn()
                }
            }
        }

        /// Returns the placeholder text for the prefix that matches the current buffer content.
        pub fn matching_placeholder_text(&self, buffer_text: &str) -> Option<String> {
            // Find exact match - buffer content must equal the prefix exactly
            self.placeholder_texts
                .iter()
                .find(|(prefix, _)| buffer_text == prefix.as_str())
                .map(|(_, text)| text.clone())
        }

        pub fn placeholder_text_exists(&self) -> bool {
            // Only consider the default placeholder (empty prefix) for "exists" checks.
            // This is used when laying out the input and determining input height.
            self.placeholder_texts.contains_key("")
        }

        pub fn is_selecting(&self, app: &AppContext) -> bool {
            self.editor_model.as_ref(app).is_selecting(app)
        }

        pub fn rightmost_point(&self, app: &AppContext) -> DisplayPoint {
            self.editor_model
                .as_ref(app)
                .display_map(app)
                .rightmost_point()
        }

        pub fn max_point(&self, app: &AppContext) -> DisplayPoint {
            self.editor_model.as_ref(app).max_point(app)
        }

        /// Lays out the given ghosted placeholder text.
        fn layout_ghosted_text(
            &self,
            text: &str,
            size: &Vector2F,
            soft_wrap: bool,
            first_line_head_indent: Option<f32>,
            font_cache: &FontCache,
            layout_cache: &LayoutCache,
        ) -> Vec<Arc<text_layout::Line>> {
            let font_size = self.font_size;

            if soft_wrap {
                let text_frame = layout_cache.layout_text(
                    text,
                    LineStyle {
                        font_size: self.font_size,
                        line_height_ratio: self.line_height_ratio,
                        baseline_ratio: DEFAULT_TOP_BOTTOM_RATIO,
                        fixed_width_tab_size: None,
                    },
                    &[(
                        0..text.chars().count(),
                        StyleAndFont::new(
                            self.placeholder_font_family,
                            self.font_properties,
                            TextStyle::new(),
                        ),
                    )],
                    size.x(),
                    f32::MAX,
                    Default::default(),
                    first_line_head_indent,
                    &font_cache.text_layout_system(),
                );

                // Return a vec of lines to match laying out the placeholder without soft wrapping.
                text_frame
                    .lines()
                    .iter()
                    .map(|l| Arc::new(l.clone()))
                    .collect()
            } else {
                text.lines()
                    .map(|line| {
                        layout_cache.layout_line(
                            line,
                            LineStyle {
                                font_size,
                                line_height_ratio: self.line_height_ratio,
                                baseline_ratio: DEFAULT_TOP_BOTTOM_RATIO,
                                fixed_width_tab_size: None,
                            },
                            &[(
                                0..line.chars().count(),
                                StyleAndFont::new(
                                    self.placeholder_font_family,
                                    self.font_properties,
                                    TextStyle::new(),
                                ),
                            )],
                            f32::MAX,
                            ClipConfig::default(),
                            &font_cache.text_layout_system(),
                        )
                    })
                    .collect()
            }
        }

        /// Layout placeholder text with an optional indent for the first line.
        pub fn layout_placeholder_text(
            &self,
            placeholder_text: &str,
            first_line_indent: f32,
            font_cache: &FontCache,
            layout_cache: &LayoutCache,
            size: &Vector2F,
            soft_wrap: bool,
        ) -> Vec<Arc<text_layout::Line>> {
            self.layout_ghosted_text(
                placeholder_text,
                size,
                soft_wrap,
                if first_line_indent > 0. {
                    Some(first_line_indent)
                } else {
                    None
                },
                font_cache,
                layout_cache,
            )
        }

        pub(super) fn clamp_scroll_left(&self, scroll_state: &ScrollState, max: f32) {
            let mut scroll_position = scroll_state.scroll_position.lock();
            let scroll_left = scroll_position.x();
            scroll_position.set_x(scroll_left.min(max));
        }

        pub fn max_scroll_top(total_lines: f32, visible_lines: f32) -> f32 {
            (total_lines - visible_lines).max(0.)
        }

        #[allow(clippy::too_many_arguments)]
        pub(super) fn autoscroll_horizontally(
            &self,
            scroll_state: &ScrollState,
            start_row: u32,
            viewport_width: f32,
            scroll_width: f32,
            max_glyph_width: f32,
            layouts: Vec<&text_layout::Line>,
            app: &AppContext,
        ) {
            let map = self.editor_model.as_ref(app).display_map(app);

            let mut target_left = f32::INFINITY;
            let mut target_right = 0.0_f32;
            for selection in self.editor_model.as_ref(app).selections(app) {
                let head = selection.head().to_display_point(map, app).unwrap();
                let start_column = head.column().saturating_sub(3);
                let end_column =
                    cmp::min(map.line_len(head.row(), app).unwrap(), head.column() + 3);

                if let Some(line) = layouts.get((head.row() - start_row) as usize) {
                    target_left = target_left.min(line.x_for_index(start_column as usize));
                }
                if let Some(line) = layouts.get((head.row() - start_row) as usize) {
                    target_right =
                        target_right.max(line.x_for_index(end_column as usize) + max_glyph_width);
                }
            }
            target_right = target_right.min(scroll_width);

            if target_right - target_left > viewport_width {
                return;
            }

            let mut scroll_position = scroll_state.scroll_position.lock();
            let scroll_left = scroll_position.x() * max_glyph_width;
            let scroll_right = scroll_left + viewport_width;

            if target_left < scroll_left {
                scroll_position.set_x(target_left / max_glyph_width);
            } else if target_right > scroll_right {
                scroll_position.set_x((target_right - viewport_width) / max_glyph_width);
            }
        }

        pub(super) fn autoscroll_vertically(
            &self,
            scroll_state: &ScrollState,
            total_lines: f32,
            visible_lines: f32,
            top_section_height_lines: f32,
            frame_layouts: &FrameLayouts,
            app: &AppContext,
        ) -> bool {
            let mut scroll_position = scroll_state.scroll_position.lock();
            let scroll_top = scroll_position.y();
            scroll_position.set_y(scroll_top.min(total_lines - visible_lines).max(0.));

            let mut autoscroll_requested = scroll_state.autoscroll_requested.lock();
            if *autoscroll_requested {
                *autoscroll_requested = false;
            } else {
                return false;
            }

            let map = self.editor_model.as_ref(app).display_map(app);

            let first_selection = self.editor_model.as_ref(app).first_selection(app);
            let first_selection_clamp_direction = first_selection.clamp_direction;
            let first_cursor = first_selection.head().to_display_point(map, app);

            let first_cursor_top = match first_cursor {
                Ok(first_cursor) => {
                    match frame_layouts
                        .to_soft_wrap_point(first_cursor, first_selection_clamp_direction)
                    {
                        Some(point) => point.row() as f32 + top_section_height_lines,
                        None => {
                            report_error!("Failed to get softwrapped point from display point");
                            return false;
                        }
                    }
                }
                Err(err) => {
                    report_error!(err.context("Error trying to turn selection into display point"));
                    return false;
                }
            };

            let last_selection = self.editor_model.as_ref(app).last_selection(app);
            let last_selection_clamp_direction = last_selection.clamp_direction;
            let last_cursor = last_selection.head().to_display_point(map, app);

            let last_cursor_bottom = match last_cursor {
                Ok(last_cursor) => {
                    match frame_layouts
                        .to_soft_wrap_point(last_cursor, last_selection_clamp_direction)
                    {
                        Some(point) => point.row() as f32 + 1.0 + top_section_height_lines,
                        None => {
                            report_error!("Failed to get softwrapped point from display point");
                            return false;
                        }
                    }
                }
                Err(err) => {
                    report_error!(err.context("Error trying to turn selection into display point"));
                    return false;
                }
            };

            let margin = ((visible_lines - (last_cursor_bottom - first_cursor_top)) / 2.0)
                .floor()
                .min(3.0);
            if margin < 0.0 {
                return false;
            }

            let target_top = (first_cursor_top - margin).max(0.0);
            let target_bottom = last_cursor_bottom + margin;
            let start_row = scroll_position.y();
            let end_row = start_row + visible_lines;

            if target_top < start_row {
                scroll_position
                    .set_y(target_top.min(Self::max_scroll_top(total_lines, visible_lines)));
            } else if target_bottom >= end_row {
                scroll_position.set_y(
                    (target_bottom - visible_lines)
                        .min(Self::max_scroll_top(total_lines, visible_lines)),
                );
            }
            true
        }

        /// If the range of rows passed does not exist, we just layout the rows that do exist
        /// within the provided range.
        pub fn layout_text_frames(
            &self,
            mut rows: Range<u32>,
            layout_cache: &LayoutCache,
            max_width: f32,
            left_notch_width_px: f32,
            app: &AppContext,
        ) -> Result<Vec<Arc<text_layout::TextFrame>>> {
            let display_map = self.editor_model.as_ref(app).display_map(app);

            rows.end = cmp::min(rows.end, display_map.max_point(app).row() + 1);
            if rows.start >= rows.end {
                return Ok(Vec::new());
            }

            let family_id = self.font_family;
            let properties = self.font_properties;
            let font_size = self.font_size;

            // Collect style information for lines
            let stylized_lines = rows
                .map(|row| {
                    display_map
                        .chars_with_styles_at(DisplayPoint::new(row, 0), app)
                        .unwrap()
                        // Stop when we hit the end of the row.
                        .take_while(|c| c.char() != '\n')
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();

            let font_cache = app.font_cache();

            let line_height_ratio = self.line_height_ratio;

            // Use rayon parallel iterators to efficiently lay out all of the lines.
            let text_layout_system = font_cache.text_layout_system();
            let layouts = stylized_lines
                .into_par_iter()
                .enumerate()
                .map(|(cur_line_idx, stylized_chars)| {
                    let mut line = String::with_capacity(stylized_chars.len());
                    let mut style_runs = Vec::with_capacity(stylized_chars.len());
                    for (idx, stylized_char) in stylized_chars.into_iter().enumerate() {
                        // Accumulate both style information and the characters to lay out.
                        style_runs.push((
                            idx..idx + 1,
                            StyleAndFont::new(family_id, properties, stylized_char.style()),
                        ));
                        line.push(stylized_char.char());
                    }

                    layout_cache.layout_text(
                        &line,
                        LineStyle {
                            font_size,
                            line_height_ratio,
                            baseline_ratio: DEFAULT_TOP_BOTTOM_RATIO,
                            fixed_width_tab_size: None,
                        },
                        &style_runs,
                        max_width,
                        f32::MAX,
                        TextAlignment::default(),
                        // Push over text by left notch width, if we're on the first line. Note notch width will be 0 if same line prompt is disabled.
                        (cur_line_idx == 0).then_some(left_notch_width_px),
                        &text_layout_system,
                    )
                })
                .collect();

            Ok(layouts)
        }

        pub fn line(&self, display_row: u32, app: &AppContext) -> Result<String> {
            self.editor_model
                .as_ref(app)
                .display_map(app)
                .line(display_row, app)
        }

        pub fn all_drawable_selections_intersecting_range<'a>(
            &'a self,
            range: Range<DisplayPoint>,
            app: &'a AppContext,
        ) -> impl 'a + Iterator<Item = DrawableSelection> {
            self.editor_model
                .as_ref(app)
                .all_drawable_selections_intersecting_range(range, app)
        }

        // todo: dedup this with EditorView. it's really: given an editor model + row, get the line len
        pub fn line_len(&self, display_row: u32, app: &AppContext) -> Result<u32> {
            self.editor_model.as_ref(app).line_len(display_row, app)
        }

        pub fn layout_line(
            &self,
            row: u32,
            text_layout_cache: &LayoutCache,
            app: &AppContext,
        ) -> Result<Arc<text_layout::Line>> {
            let font_cache = app.font_cache();
            let line = self.line(row, app)?;

            Ok(text_layout_cache.layout_line(
                &line,
                LineStyle {
                    font_size: self.font_size,
                    line_height_ratio: self.line_height_ratio,
                    baseline_ratio: DEFAULT_TOP_BOTTOM_RATIO,
                    fixed_width_tab_size: None,
                },
                &[(
                    0..self.line_len(row, app)? as usize,
                    StyleAndFont::new(self.font_family, Default::default(), TextStyle::new()),
                )],
                f32::MAX,
                ClipConfig::default(),
                &font_cache.text_layout_system(),
            ))
        }
    }
}

mod movement {
    use anyhow::Result;
    use warpui::AppContext;

    use super::{DisplayMap, DisplayPoint};

    pub fn left(
        map: &DisplayMap,
        mut point: DisplayPoint,
        app: &AppContext,
        stop_at_line_start: bool,
    ) -> Result<DisplayPoint> {
        if point.column() > 0 {
            *point.column_mut() -= 1;
        } else if !stop_at_line_start && point.row() > 0 {
            *point.row_mut() -= 1;
            *point.column_mut() = map.line_len(point.row(), app)?;
        }
        Ok(point)
    }

    pub fn right(
        map: &DisplayMap,
        mut point: DisplayPoint,
        app: &AppContext,
        stop_at_line_end: bool,
    ) -> Result<DisplayPoint> {
        let max_column = map.line_len(point.row(), app).unwrap();
        if point.column() < max_column {
            *point.column_mut() += 1;
        } else if !stop_at_line_end && point.row() < map.max_point(app).row() {
            *point.row_mut() += 1;
            *point.column_mut() = 0;
        }
        Ok(point)
    }
}

mod model {
    use std::collections::{HashMap, HashSet};
    use std::ops::Range;

    pub use buffer::{
        Anchor, Chars, EditOrigin, TextRun, TextStyleOperation, ToBufferOffset, ToCharOffset,
        ToPoint,
    };
    use buffer::{Buffer, Text};
    pub use display_map::{Bias, DisplayMap, DisplayPoint, MovementResult, ToDisplayPoint};
    use itertools::Itertools;
    use lazy_static::lazy_static;
    use num_traits::SaturatingSub;
    pub use selections::{
        DrawableSelection, LocalDrawableSelectionData, LocalPendingSelection, LocalSelection,
        LocalSelections, MarkedTextState, SelectAction, Selection, SelectionMode,
    };
    use string_offset::{ByteOffset, CharOffset};
    use vec1::{Vec1, vec1};
    use warp_errors::report_error;
    use warpui::accessibility::{AccessibilityContent, WarpA11yRole};
    use warpui::text::TextBuffer;
    use warpui::text::point::Point;
    use warpui::text::word_boundaries::WordBoundariesPolicy;
    use warpui::text_layout::TextStyle;
    use warpui::{AppContext, Entity, ModelAsRef, ModelContext, ModelHandle};

    use super::{PlainTextEditorViewAction, SelectionInsertion, ValidInputType, movement};

    lazy_static! {
        static ref AUTOCOMPLETE_SYMBOLS: HashMap<&'static str, &'static str> = HashMap::from([
            ("(", ")"),
            ("[", "]"),
            ("{", "}"),
            ("\'", "\'"),
            ("\"", "\""),
        ]);
        static ref CLOSING_SYMBOLS: HashSet<&'static str> =
            AUTOCOMPLETE_SYMBOLS.values().cloned().collect();
    }

    /// A snapshot of the editor that does not expose
    /// buffer implementation details.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct EditorSnapshot {
        /// When taking a snapshot, we intentionally use char offsets.
        /// We can't use the [`LocalSelection`] type because those rely
        /// on anchors which are associated to edit IDs. Using char offsets
        /// allows us to ensure we can restore selection state _across_
        /// buffers (e.g. ephemeral -> regular).
        selections: Vec1<Range<CharOffset>>,
        /// Similar to the reason we use offsets instead of anchors
        /// for [`Self::selections`], we use [`TextRun`]s instead
        /// of [`Fragment`]s or similar to describe the buffer text.
        buffer_text_runs: Vec<TextRun>,
    }

    impl EditorSnapshot {
        fn new(selections: Vec1<Range<CharOffset>>, buffer_text_runs: Vec<TextRun>) -> Self {
            Self {
                selections,
                buffer_text_runs,
            }
        }

        fn buffer_text_runs(&self) -> &Vec<TextRun> {
            &self.buffer_text_runs
        }

        pub fn text(&self) -> String {
            TextRun::text_from_text_runs(&self.buffer_text_runs)
        }

        pub fn selections(&self) -> &Vec1<Range<CharOffset>> {
            &self.selections
        }
    }

    /// Enum representing editor responses to edit and selection actions.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum InteractionState {
        /// Editor will respond to both edits and selection actions; cursor shown.
        Editable,
        /// Editor was set in an editable state, but has selections that cannot be edited.
        EditableWithInvalidSelection,
        /// Editor will respond to selection actions, and ignore edit actions. No cursor is shown.
        Selectable,
        /// Editor will ignore selection and edit actions. Editor is rendered with dimmed text and no cursor.
        Disabled,
    }

    type StaticMutableModelCallback = fn(&mut EditorModel, &mut ModelContext<EditorModel>);
    type StaticCallback = fn(&EditorModel, &mut ModelContext<EditorModel>);

    #[derive(Copy, Clone)]
    pub enum UpdateBufferOption {
        /// No special behaviour.
        None,

        /// The edits are not recorded on the undo / redo stack.
        #[allow(dead_code)]
        SkipUndoRedoRecord,

        /// The edits are recorded in a dedicated, ephemeral buffer
        /// so as not to change the real buffer.
        /// Ephemeral edits can be materialized as soon as a non-ephemeral edit
        /// is made. This is done by replacing the real buffer with the ephemeral
        /// buffer before the non-ephemeral edit is applied.
        IsEphemeral,
    }

    impl UpdateBufferOption {
        fn skip_undo(&self) -> bool {
            matches!(self, Self::SkipUndoRedoRecord | Self::IsEphemeral)
        }
    }

    /// A helper struct to group together state needed
    /// to specify how the buffer should be updated.
    struct UpdateBuffer<U>
    where
        U: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
    {
        action: PlainTextEditorViewAction,
        edit_origin: EditOrigin,
        option: UpdateBufferOption,
        callback: U,
    }

    impl<U> UpdateBuffer<U>
    where
        U: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
    {
        fn skip_undo(&self) -> bool {
            self.option.skip_undo()
        }
    }

    /// Helper struct to change the underlying buffer via the model.
    /// By edit, we mean string manipulation of the buffer or selection changes.
    ///
    /// The view does not have access to the buffer so instead we expose
    /// this struct for the view to declare how the buffer should be changed
    /// while the model is responsible for actually changing the buffer
    /// and updating corresponding state.
    ///
    /// There are 4 available, optional callbacks. They are executed in the following order:
    /// 1) change_selection_callback
    /// 2) before_buffer_edit_callback
    /// 3) update_buffer_callback
    /// 4) post_buffer_edit_change_selection_callback
    ///
    /// The buffer changes specified by a single `Edits` instance
    /// are considered atomic / batched. See [`EditorModel::edit`] for how this struct is used.
    ///
    /// TODO (suraj): technically, there's nothing enforcing that these callbacks
    /// only do what they're described to do (e.g. the change_selection callback
    /// can edit the buffer). To enforce this, we should only expose what needs
    /// to be exposed in the respective callbacks. For example, the selection-related
    /// callbacks only need the mutable selection state in the callback (not the
    /// whole mutable model state). Similarly, for the edit callback, we only need
    /// the mutable Buffer. To keep using the single API, we can have the editor model
    /// helper APIs return callbacks that can then be used as the callbacks in `Edits`.
    /// The view APIs can use this to compose an edit.
    pub struct Edits<C, B, U, G>
    where
        C: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
        B: FnOnce(&EditorModel, &mut ModelContext<EditorModel>),
        U: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
        G: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
    {
        change_selections_callback: Option<C>,
        before_buffer_edit_callback: Option<B>,
        update_buffer: Option<UpdateBuffer<U>>,
        post_buffer_edit_change_selections_callback: Option<G>,
    }

    impl
        Edits<
            StaticMutableModelCallback,
            StaticCallback,
            StaticMutableModelCallback,
            StaticMutableModelCallback,
        >
    {
        #[allow(clippy::new_without_default)]
        pub fn new() -> Self {
            Self {
                change_selections_callback: None,
                before_buffer_edit_callback: None,
                update_buffer: None,
                post_buffer_edit_change_selections_callback: None,
            }
        }
    }

    impl<C, B, U, G> Edits<C, B, U, G>
    where
        C: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
        B: FnOnce(&EditorModel, &mut ModelContext<EditorModel>),
        U: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
        G: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
    {
        /// Allows callers to specify a callback to update the selection state
        /// as part of an edit.
        pub fn with_change_selections<T>(self, change_selection: T) -> Edits<T, B, U, G>
        where
            T: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
        {
            Edits {
                change_selections_callback: Some(change_selection),
                before_buffer_edit_callback: self.before_buffer_edit_callback,
                update_buffer: self.update_buffer,
                post_buffer_edit_change_selections_callback: self
                    .post_buffer_edit_change_selections_callback,
            }
        }

        /// Allows callers to specify a callback to update the buffer
        /// as part of an edit.
        pub fn with_update_buffer<T>(
            self,
            action: PlainTextEditorViewAction,
            edit_origin: EditOrigin,
            callback: T,
        ) -> Edits<C, B, T, G>
        where
            T: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
        {
            let update_buffer = UpdateBuffer {
                action,
                edit_origin,
                option: UpdateBufferOption::None,
                callback,
            };
            Edits {
                change_selections_callback: self.change_selections_callback,
                before_buffer_edit_callback: self.before_buffer_edit_callback,
                update_buffer: Some(update_buffer),
                post_buffer_edit_change_selections_callback: self
                    .post_buffer_edit_change_selections_callback,
            }
        }

        /// Allows callers to specify a callback to update the buffer
        /// as part of an edit, without adding an entry to the undo / redo stack.
        ///
        /// This should be used very sparingly. Prefer to use
        /// [`Self::with_update_buffer`].
        pub fn with_update_buffer_options<T>(
            self,
            action: PlainTextEditorViewAction,
            edit_origin: EditOrigin,
            option: UpdateBufferOption,
            callback: T,
        ) -> Edits<C, B, T, G>
        where
            T: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
        {
            let update_buffer = UpdateBuffer {
                action,
                edit_origin,
                option,
                callback,
            };
            Edits {
                change_selections_callback: self.change_selections_callback,
                before_buffer_edit_callback: self.before_buffer_edit_callback,
                update_buffer: Some(update_buffer),
                post_buffer_edit_change_selections_callback: self
                    .post_buffer_edit_change_selections_callback,
            }
        }

        fn is_pure_selection_change(&self) -> bool {
            self.update_buffer.is_none() && self.includes_selection_change()
        }

        fn includes_selection_change(&self) -> bool {
            self.change_selections_callback.is_some()
                || self.post_buffer_edit_change_selections_callback.is_some()
        }

        fn is_ephemeral(&self) -> bool {
            self.update_buffer
                .as_ref()
                .is_some_and(|u| matches!(u.option, UpdateBufferOption::IsEphemeral))
        }
    }

    struct BufferAndDisplayMaps {
        /// The main buffer and display map.
        regular: (ModelHandle<Buffer>, ModelHandle<DisplayMap>),

        /// A buffer and display map dedicated for ephemeral edits (see [`UpdateBufferOption::IsEphemeral`]).
        /// If [`Some`], then the ephemeral buffer is active.
        ephemeral: Option<(ModelHandle<Buffer>, ModelHandle<DisplayMap>)>,
    }

    impl BufferAndDisplayMaps {
        /// Returns a [`ModelHandle`] to the active [`Buffer`].
        fn buffer_handle(&self) -> &ModelHandle<Buffer> {
            if let Some((buffer, _)) = self.ephemeral.as_ref() {
                buffer
            } else {
                &self.regular.0
            }
        }

        /// Returns a [`ModelHandle`] to the active [`DisplayMap`].
        fn display_map_handle(&self) -> &ModelHandle<DisplayMap> {
            if let Some((_, display_map)) = self.ephemeral.as_ref() {
                display_map
            } else {
                &self.regular.1
            }
        }

        /// Deactivates any ephemeral state.
        fn deactivate_ephemeral_state(&mut self) {
            self.ephemeral.take();
        }

        /// Activates a new regular ephemeral state whose content will be applied
        /// to the regular buffer when the ephemeral is exited (materialized).
        fn activate_new_ephemeral_state(&mut self, ctx: &mut ModelContext<EditorModel>) {
            let tab_size = self.regular.1.as_ref(ctx).tab_size();
            let ephemeral_buffer = ctx.add_model(|_| Buffer::new(""));
            let ephemeral_display_map: ModelHandle<DisplayMap> =
                ctx.add_model(|ctx| DisplayMap::new(ephemeral_buffer.clone(), tab_size, ctx));
            ctx.subscribe_to_model(&ephemeral_buffer, EditorModel::handle_buffer_event);
            ctx.subscribe_to_model(
                &ephemeral_display_map,
                EditorModel::handle_display_map_event,
            );
            self.ephemeral = Some((ephemeral_buffer, ephemeral_display_map));
        }
    }

    pub struct EditorModel {
        /// The extent to which the editor is interactable.
        interaction_state: InteractionState,

        /// The [`Buffer`]s and [`DisplayMap`]s used by this editor.
        /// Use [`Self::buffer`] and [`Self::display_map`] to access the
        /// correct buffer and display map, respectively.
        buffer_and_display_map: BufferAndDisplayMaps,

        /// Counter for recording whether the last editor action contains an autocompleted symbol.
        /// This increments with each consecutive autocompleted insertion.
        consecutive_autocomplete_insertion_edits_counter: usize,

        /// The maximum buffer length for the editor. Any operations that cause the buffer to exceed
        /// this length will be rejected.
        max_buffer_len: Option<usize>,

        /// The types of inputs that are valid for this editor.
        valid_input_type: ValidInputType,

        /// The buffer text before the last edit / undo / redo.
        last_buffer_text: String,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum EditorModelEvent {
        Edited {
            edit_origin: EditOrigin,
        },
        StylesUpdated,
        DisplayMapUpdated,
        SelectionsChanged,
        ShellCut(String),
        /// An edit action has replaced the entire buffer's content.
        BufferReplaced,
    }

    impl Entity for EditorModel {
        type Event = EditorModelEvent;
    }

    /// The public interface.
    impl EditorModel {
        /// Helper method to emit a11y event when the selections change. For now, only single cursor
        /// selections are supported.
        /// The app will emit values based on the following matrix:
        ///
        /// | Start            | End              | What should be read     |
        /// |--------------    |--------------    |---------------------    |
        /// | No selection     | No selection     | Delta (characters)      |
        /// | No selection     | Selection        | "Selected: {delta}"     |
        /// | Selection        | Selection        | "Selected: {delta}"     |
        /// | Selection        | No selection     | "Unselected"            |
        ///
        pub fn delta_for_a11y(
            &self,
            selection_before: Range<ByteOffset>,
            was_selecting: bool,
            ctx: &mut ModelContext<Self>,
        ) -> AccessibilityContent {
            let selection_after = self.first_selection(ctx).to_byte_offset(self.buffer(ctx));
            let is_selecting = !self.first_selection(ctx).is_cursor_only(self.buffer(ctx));

            let text = self.buffer(ctx).text();

            // Note: is_empty on a range means that start >= end
            // We use it to verify whether the direction of the selection has changed
            let (before_point, after_point) =
                if selection_before.is_empty() == selection_after.is_empty() {
                    (selection_before.start, selection_after.start)
                } else {
                    // we changed direction when selecting
                    (selection_before.end, selection_after.start)
                };
            let (start, end) = if before_point < after_point {
                (before_point, after_point)
            } else {
                (after_point, before_point)
            };
            let delta = &text[start.as_usize()..end.as_usize()];
            match (was_selecting, is_selecting) {
                (false, false) => {
                    AccessibilityContent::new_without_help(delta, WarpA11yRole::UserAction)
                }
                (_, true) => {
                    // Note that Range is start <= x < end, and in our case, when deciding what was the action
                    // (selecting or unselecting) we need to take end element into consideration also,
                    // hence the helper `inclusive_contains` method.
                    fn inclusive_contains(
                        selection: &Range<ByteOffset>,
                        element: ByteOffset,
                    ) -> bool {
                        selection.contains(&element) || selection.end == element
                    }
                    // Since the range here may show as empty, we're swapping it, so that it's (n, m), where
                    // n<m
                    let selection_after = if selection_after.is_empty() {
                        selection_after.end..selection_after.start
                    } else {
                        selection_after
                    };

                    let action = if inclusive_contains(&selection_after, start)
                        && inclusive_contains(&selection_after, end)
                    {
                        "selected"
                    } else {
                        "unselected"
                    };
                    AccessibilityContent::new(
                        delta,
                        format!(", {action}"),
                        WarpA11yRole::UserAction,
                    )
                }
                (true, false) => {
                    AccessibilityContent::new_without_help("Unselected", WarpA11yRole::UserAction)
                }
            }
        }

        pub fn new(
            base_text: String,
            tab_size: usize,
            max_buffer_len: Option<usize>,
            valid_input_type: ValidInputType,
            ctx: &mut ModelContext<Self>,
        ) -> Self {
            let regular_buffer = ctx.add_model(|_| Buffer::new(base_text.clone()));
            let regular_display_map =
                ctx.add_model(|ctx| DisplayMap::new(regular_buffer.clone(), tab_size, ctx));
            ctx.subscribe_to_model(&regular_buffer, Self::handle_buffer_event);
            ctx.subscribe_to_model(&regular_display_map, Self::handle_display_map_event);
            let regular = (regular_buffer, regular_display_map);

            Self {
                interaction_state: InteractionState::Editable,
                buffer_and_display_map: BufferAndDisplayMaps {
                    regular,
                    ephemeral: None,
                },
                consecutive_autocomplete_insertion_edits_counter: 0,
                max_buffer_len,
                valid_input_type,
                last_buffer_text: base_text,
            }
        }

        fn refresh_batch_version(&mut self, ctx: &mut ModelContext<Self>) {
            self.buffer_handle().update(ctx, |buffer, _| {
                buffer.refresh_version_on_edits_and_selection_changes_batch()
            });
        }

        fn start_batch<U>(&mut self, update: Option<&UpdateBuffer<U>>, ctx: &mut ModelContext<Self>)
        where
            U: FnOnce(&mut EditorModel, &mut ModelContext<EditorModel>),
        {
            if let Some(update) = update {
                if !self.is_ephemeral() {
                    self.last_buffer_text = self.buffer_text(ctx);
                }

                self.buffer_handle().update(ctx, |buffer, _| {
                    buffer.start_edits_and_selection_changes_batch(
                        update.edit_origin,
                        update.action,
                        update.skip_undo(),
                    );
                });
            } else {
                self.buffer_handle().update(ctx, |buffer, _| {
                    buffer.start_selection_changes_only_batch();
                });
            }
        }

        fn end_batch(&mut self, ctx: &mut ModelContext<Self>) {
            self.buffer_handle().update(ctx, |buffer, ctx| {
                buffer.end_batch(ctx);
            });
        }

        pub fn change_selections(
            &mut self,
            new_selections: impl Into<LocalSelections>,
            ctx: &mut ModelContext<Self>,
        ) {
            self.buffer_handle().update(ctx, |buffer, _| {
                if let Err(e) = buffer.change_selections(new_selections.into()) {
                    log::warn!("Failed to change selections: {e}");
                }
            });
        }

        /// The main API to update the underlying buffer. View APIs
        /// should never attempt to update the buffer state without going
        /// through this API. There are guards in place to ensure we are using
        /// this API when updating the buffer in a controlled fashion.
        ///
        /// Selection changes are only applied if the editor is in a selectable state.
        /// Buffer changes are only applied if the editor is in an editable state.
        pub fn edit<F, G, H, M>(&mut self, ctx: &mut ModelContext<Self>, edit: Edits<F, G, H, M>)
        where
            F: FnOnce(&mut Self, &mut ModelContext<Self>),
            G: FnOnce(&Self, &mut ModelContext<Self>),
            H: FnOnce(&mut Self, &mut ModelContext<Self>),
            M: FnOnce(&mut Self, &mut ModelContext<Self>),
        {
            let can_select = self.can_select();
            let can_edit = self.can_edit();
            let first_cursor_before = self.first_selection(ctx).to_byte_offset(self.buffer(ctx));
            let was_selecting = !self.first_selection(ctx).is_cursor_only(self.buffer(ctx));
            let is_pure_selection_change = can_select && edit.is_pure_selection_change();

            // If this is an ephemeral edit, then snapshot the current buffer
            // so that the ephemeral edit is applied on the latest buffer state
            // (whether that's the real buffer or the last ephemeral buffer).
            // This needs to be done _before_ we start the batch below so that
            // 1) we take the snapshot of the correct (regular vs. ephemeral) buffer
            // 2) we star the the batch on the correct (regular vs. ephemeral) buffer
            let restore_from_snapshot = if can_edit && edit.is_ephemeral() {
                let snapshot = self.as_snapshot(ctx);
                self.buffer_and_display_map
                    .activate_new_ephemeral_state(ctx);
                Some(snapshot)
            } else if can_edit && self.is_ephemeral() && edit.update_buffer.is_some() {
                // Snapshot the ephemeral buffer so its content can be applied to the regular buffer.
                let snapshot = self.as_snapshot(ctx);
                self.buffer_and_display_map.deactivate_ephemeral_state();
                Some(snapshot)
            } else {
                None
            };

            // Start the batch after setting the ephemeral state so that
            // the batch is started on the right buffer.
            // Note: now that we've started a batch, it must be ended.
            // We cannot return early in this function.
            self.start_batch(edit.update_buffer.as_ref(), ctx);

            // If we need to restore between regular and ephemeral states,
            // do it now so that any selections / edits beyond are applied to the
            // right buffer state. For example, if we tried to select
            // without having restored the snapshot, we would be selecting
            // on the wrong underlying buffer.
            if let Some(snapshot) = restore_from_snapshot {
                self.restore_from_snapshot(snapshot, ctx);
                // Refresh batch version here as we already recorded edits for the snapshot
                // restoration.
                self.refresh_batch_version(ctx);

                // Do a custom undo / redo record without exhausting the batched edits.
                // This should be fine because there shouldn't have been any other edits so far.
                self.buffer_handle().update(ctx, |buffer, ctx| {
                    buffer.record_edits(PlainTextEditorViewAction::ReplaceBuffer, ctx);
                });
            }

            if can_select && let Some(change_selection) = edit.change_selections_callback {
                change_selection(self, ctx);
            }

            if let Some(before_buffer_edit) = edit.before_buffer_edit_callback {
                before_buffer_edit(self, ctx);
            }

            let prev_state = self.consecutive_autocomplete_insertion_edits_counter;
            if can_edit && let Some(update_buffer) = edit.update_buffer {
                (update_buffer.callback)(self, ctx);
            }
            let next_state = self.consecutive_autocomplete_insertion_edits_counter;

            if can_select
                && let Some(post_buffer_edit_change_selection) =
                    edit.post_buffer_edit_change_selections_callback
            {
                post_buffer_edit_change_selection(self, ctx);
            }

            // Only emit a11y content for pure selection changes because edits
            // have their own a11y content already.
            // TODO: technically, a pure selection change can also happen via the `update_buffer` API.
            // We might want to actually do this similar to how we emit selection changes (i.e. in `Buffer::end_batch`).
            if is_pure_selection_change {
                let a11y_content = self.delta_for_a11y(first_cursor_before, was_selecting, ctx);
                ctx.emit_a11y_content(a11y_content);
            }

            // Check if the last edit inserted a new autocomplete symbol.
            if prev_state == next_state {
                self.consecutive_autocomplete_insertion_edits_counter = 0;
            }

            self.end_batch(ctx);
        }

        pub fn interaction_state(&self) -> InteractionState {
            self.interaction_state
        }

        pub fn set_interaction_state(&mut self, interaction_state: InteractionState) {
            self.interaction_state = interaction_state;
        }

        pub fn can_edit(&self) -> bool {
            matches!(self.interaction_state, InteractionState::Editable)
        }

        pub fn can_select(&self) -> bool {
            matches!(
                self.interaction_state,
                InteractionState::Selectable
                    | InteractionState::Editable
                    | InteractionState::EditableWithInvalidSelection
            )
        }

        pub fn undo(&mut self, ctx: &mut ModelContext<Self>) {
            if !self.can_edit() {
                return;
            }

            if self.is_ephemeral() {
                self.buffer_and_display_map.deactivate_ephemeral_state();
                ctx.emit(EditorModelEvent::Edited {
                    edit_origin: EditOrigin::UserInitiated,
                });
                return;
            }

            self.last_buffer_text = self.buffer_text(ctx);
            self.buffer_handle().update(ctx, |buffer, ctx| {
                buffer.undo(ctx);
                ctx.notify();
            });
        }

        pub fn redo(&mut self, ctx: &mut ModelContext<Self>) {
            if !self.can_edit() {
                return;
            }

            self.last_buffer_text = self.buffer_text(ctx);
            self.buffer_handle().update(ctx, |buffer, ctx| {
                buffer.redo(ctx);
                ctx.notify();
            });
        }

        pub fn reset_undo_redo_stack(&mut self, ctx: &mut ModelContext<Self>) {
            self.buffer_handle().update(ctx, |buffer, _ctx| {
                buffer.reset_undo_stack();
            })
        }

        pub fn consecutive_autocomplete_insertion_edits_counter(&self) -> usize {
            self.consecutive_autocomplete_insertion_edits_counter
        }

        pub fn as_snapshot<C: ModelAsRef>(&self, ctx: &C) -> EditorSnapshot {
            self.buffer(ctx).snapshot()
        }

        pub fn buffer_edit<I, S, T>(
            &mut self,
            old_ranges: I,
            new_text: T,
            ctx: &mut ModelContext<Self>,
        ) -> anyhow::Result<()>
        where
            I: IntoIterator<Item = Range<S>>,
            S: ToCharOffset,
            T: Into<Text>,
        {
            self.buffer_handle().update(ctx, |buffer, ctx| {
                buffer.edit(old_ranges, new_text, ctx).map(|_| ())
            })
        }

        pub fn selected_text<C: ModelAsRef>(&self, ctx: &C) -> String {
            let buffer = self.buffer(ctx);
            let last_ix = self.selections(ctx).len() - 1;
            self.selections(ctx)
                .iter()
                .enumerate()
                .flat_map(|(ix, selection)| {
                    let start = selection.start().to_char_offset(buffer).unwrap();
                    let end = selection.end().to_char_offset(buffer).unwrap();
                    buffer
                        .chars_at(start)
                        .unwrap()
                        .take(end.as_usize() - start.as_usize())
                        .chain(if ix < last_ix { Some('\n') } else { None })
                })
                .collect()
        }

        pub fn selected_text_strings(&self, ctx: &AppContext) -> Vec<String> {
            let buffer = self.buffer(ctx);
            self.selections(ctx)
                .iter()
                .map(|selection| {
                    let start = selection
                        .start()
                        .to_char_offset(buffer)
                        .expect("Start character in selection buffer should exist");
                    let end = selection
                        .end()
                        .to_char_offset(buffer)
                        .expect("End character in selection buffer should exist");
                    buffer
                        .chars_at(start)
                        .expect("Buffer should contain character at start index of selection")
                        .take(end.as_usize().saturating_sub(start.as_usize()))
                        .collect()
                })
                .collect()
        }

        pub fn indent(&mut self, ctx: &mut ModelContext<Self>) {
            let start = self.first_selection(ctx).start().to_point(self.buffer(ctx));
            let end = self.first_selection(ctx).end().to_point(self.buffer(ctx));
            if let Some((start, end)) = start.ok().zip(end.ok()) {
                self.buffer_handle().update(ctx, |buffer, ctx| {
                    if let Err(error) = buffer.indent(start.row..end.row + 1, ctx) {
                        report_error!(error.context("error indenting text"));
                    }
                });
            }
        }

        pub fn unindent(&mut self, ctx: &mut ModelContext<Self>) {
            self.buffer_handle().update(ctx, |buffer, ctx| {
                // Unlike indent, unindent is applied always: to multiple selections and even single characters/words.
                let row_ranges = buffer
                    .local_selections()
                    .selections
                    .iter()
                    .map(|selection| {
                        selection.start().to_point(buffer).unwrap().row
                            ..selection.end().to_point(buffer).unwrap().row + 1
                    })
                    .dedup()
                    .collect::<Vec<_>>();

                if let Err(error) = buffer.unindent(row_ranges, ctx) {
                    report_error!(error.context("error unindenting text"));
                };
            });
        }

        pub fn insert(
            &mut self,
            text: &str,
            text_style: Option<TextStyle>,
            ctx: &mut ModelContext<Self>,
        ) {
            self.insert_internal(text, text_style, SelectionInsertion::No, ctx);
        }

        pub fn insert_selected_text(&mut self, text: &str, ctx: &mut ModelContext<Self>) {
            self.insert_internal(text, None, SelectionInsertion::Yes, ctx);
        }

        pub fn clear_all_styles(&self, ctx: &mut ModelContext<Self>) {
            let len = self.buffer_text(ctx).len();
            let full_range = ByteOffset::from(0)..ByteOffset::from(len);
            self.update_buffer_styles([full_range], TextStyleOperation::clear_all(), ctx);
        }

        pub fn update_buffer_styles<I, S>(
            &self,
            old_ranges: I,
            text_style_operation: TextStyleOperation,
            ctx: &mut ModelContext<Self>,
        ) where
            I: IntoIterator<Item = Range<S>>,
            S: ToCharOffset,
        {
            self.buffer_handle().update(ctx, |buffer, ctx| {
                if let Err(e) = buffer.update_styles(old_ranges, text_style_operation, ctx) {
                    report_error!(e.context("Error with updating styles"))
                }
            });
        }

        /// Autocomplete user inserted text under the following conditions:
        /// 1) The symbol has a corresponding closing symbol (e.g. ( -> ), " -> ", etc)
        /// 2) All of the current cursors are not immediately next to any alphanumeric character
        ///
        // If there are only selections (no single cursor), autocomplete by wrapping the selection with start and end symbol.
        pub fn insert_and_maybe_autocomplete_symbols(
            &mut self,
            text: &str,
            ctx: &mut ModelContext<Self>,
        ) {
            if CLOSING_SYMBOLS.contains(&text)
                && text.chars().count() == 1
                && self.all_cursors_next_character_matches_char(
                    text.chars()
                        .next()
                        .expect("Autocompleted symbol should have at least one character"),
                    ctx,
                )
            {
                let map = self.display_map(ctx);

                // Moves cursor to the right.
                let mut new_selections = self.selections(ctx).clone();
                for selection in new_selections.iter_mut() {
                    let end = selection.end().to_display_point(map, ctx).unwrap();
                    let cursor = map
                        .anchor_before(
                            movement::right(map, end, ctx, false)
                                .expect("moving right should return a valid DisplayPoint"),
                            Bias::Right,
                            ctx,
                        )
                        .expect("DisplayPoint should convert to an Anchor");

                    selection.set_start(cursor.clone());
                    selection.set_end(cursor);
                    selection.set_reversed(false);
                    selection.goal_start_column = None;
                    selection.goal_end_column = None;
                }
                self.change_selections(new_selections, ctx);
                return;
            }

            // Autocomplete for symbols like double quotes and brackets.
            if let Some(completion) = AUTOCOMPLETE_SYMBOLS.get(text) {
                if self.no_cursor_only_selections(ctx) {
                    self.insert_characters_wrap_around_selection(text, completion, ctx);
                    return;
                }

                let buffer = self.buffer(ctx);
                // Check whether all of the current cursors are not immediately next to any alphanumeric characters.
                let not_alphanumeric = self.selections(ctx).iter().all(|selection| {
                    let position = selection
                        .start()
                        .to_point(buffer)
                        .expect("Start of selection should exist");

                    let right_not_alphanumeric = buffer
                        .chars_at(position)
                        .unwrap()
                        .next()
                        .map(|right_char| !right_char.is_alphanumeric())
                        .unwrap_or(true);
                    let left_not_alphanumeric = buffer
                        .chars_at(position)
                        .unwrap()
                        .rev()
                        .next()
                        .map(|left_char| !left_char.is_alphanumeric())
                        .unwrap_or(true);
                    right_not_alphanumeric && left_not_alphanumeric
                });

                // Always autocomplete for brackets and parentheses. For quotes, only autocomplete
                // if both sides of the cursor are not alphabetic or numeric characters.
                if (text != "\"" && text != "\'") || not_alphanumeric {
                    self.insert_internal(text, None, SelectionInsertion::No, ctx);
                    let offset_ranges = self.selection_to_char_offset_ranges(ctx);

                    // Complete the closing symbol without moving the cursor.
                    self.buffer_handle().update(ctx, |buffer, ctx| {
                        if let Err(error) =
                            buffer.edit(offset_ranges.iter().cloned(), *completion, ctx)
                        {
                            report_error!(error.context("error inserting text"));
                        };
                    });
                    self.consecutive_autocomplete_insertion_edits_counter += 1;
                    return;
                }
            }

            self.insert_internal(text, None, SelectionInsertion::No, ctx);
        }

        pub fn insert_internal(
            &mut self,
            text: &str,
            text_style: Option<TextStyle>,
            select_insertion: SelectionInsertion,
            ctx: &mut ModelContext<Self>,
        ) {
            let offset_ranges = self.selection_to_char_offset_ranges(ctx);
            if self.text_insertion_would_exceed_max_buffer_len(text, ctx) {
                return;
            }
            if self.text_is_invalid_input_type(text) {
                return;
            }
            let old_buffer_count = self.buffer_len(ctx);

            self.buffer_handle().update(ctx, |buffer, ctx| {
                if let Err(error) = buffer.edit(
                    offset_ranges.iter().cloned(),
                    Text::new(text, text_style),
                    ctx,
                ) {
                    report_error!(error.context("error inserting text"));
                };
            });

            let char_count = text.chars().count() as isize;
            let mut delta = 0_isize;
            let mut total_deleted = 0_isize;

            let buffer = self.buffer(ctx);
            let new_selections = offset_ranges.mapped(|range| {
                let range_start = range.start.as_usize() as isize;
                let range_end = range.end.as_usize() as isize;
                let end = buffer
                    .anchor_before(CharOffset::from(
                        (range_start + delta + char_count) as usize,
                    ))
                    .unwrap();
                let start = match select_insertion {
                    SelectionInsertion::Yes => buffer
                        .anchor_before(CharOffset::from((range_start + delta) as usize))
                        .unwrap(),
                    SelectionInsertion::No => end.clone(),
                };
                let deleted_count = range_end - range_start;
                total_deleted += deleted_count;
                delta += char_count - deleted_count;
                LocalSelection {
                    selection: Selection {
                        start,
                        end,
                        reversed: false,
                    },
                    clamp_direction: Default::default(),
                    goal_start_column: None,
                    goal_end_column: None,
                }
            });

            if total_deleted as usize == old_buffer_count.as_usize() {
                ctx.emit(EditorModelEvent::BufferReplaced);
            }
            self.change_selections(new_selections, ctx);
        }

        pub fn marked_text_state(&self, ctx: &AppContext) -> MarkedTextState {
            self.buffer(ctx).marked_text_state()
        }

        pub fn update_marked_text(
            &mut self,
            text: &str,
            marked_text_style: Option<TextStyle>,
            selected_range: &Range<usize>,
            ctx: &mut ModelContext<Self>,
        ) {
            let marked_text_state = self.marked_text_state(ctx);
            // If there was no marked text before, then we should replace each selection with blank text.
            if marked_text_state == MarkedTextState::Inactive {
                self.insert_internal("", None, SelectionInsertion::No, ctx);
            }

            // Insert the marked text in all selections as selected text.
            self.insert_internal(text, marked_text_style, SelectionInsertion::Yes, ctx);
            self.set_marked_text_state(
                MarkedTextState::Active {
                    selected_range: selected_range.clone(),
                },
                ctx,
            );
        }

        fn set_marked_text_state(
            &mut self,
            marked_text_state: MarkedTextState,
            ctx: &mut ModelContext<Self>,
        ) {
            self.buffer_handle().update(ctx, |buffer, _ctx| {
                buffer.set_marked_text_state(marked_text_state);
            });
        }

        pub fn commit_incomplete_marked_text(&mut self, ctx: &mut ModelContext<Self>) {
            // We assume that all "selections" are actually the composed marked text.
            let selected_strings = self.selected_text_strings(ctx);
            let incomplete_marked_text = selected_strings.first().map(|s| s.as_str()).unwrap_or("");
            self.clear_marked_text_and_commit(incomplete_marked_text, ctx);
        }

        pub fn clear_marked_text_and_commit(&mut self, text: &str, ctx: &mut ModelContext<Self>) {
            self.insert_internal(text, None, SelectionInsertion::No, ctx);
            self.set_marked_text_state(MarkedTextState::Inactive, ctx);
        }

        pub fn clear_marked_text(&mut self, ctx: &mut ModelContext<Self>) {
            if self.marked_text_state(ctx) == MarkedTextState::Inactive {
                return;
            }
            self.edit(
                ctx,
                Edits::new().with_update_buffer_options(
                    PlainTextEditorViewAction::UpdateMarkedText,
                    EditOrigin::UserTyped,
                    UpdateBufferOption::None,
                    |editor_model, ctx| {
                        editor_model.insert_internal("", None, SelectionInsertion::No, ctx);
                        editor_model.set_marked_text_state(MarkedTextState::Inactive, ctx);
                    },
                ),
            );
        }

        // Used for restoring the buffer from a snapshot.
        fn restore_from_snapshot(
            &mut self,
            snapshot: EditorSnapshot,
            ctx: &mut ModelContext<Self>,
        ) {
            let version = self.buffer_handle().as_ref(ctx).versions();
            self.clear_selections(ctx);
            self.clear_buffer(ctx);

            let mut text_added_offset = CharOffset::from(0);
            self.buffer_handle().update(ctx, |buffer, ctx| {
                for styled_text in snapshot.buffer_text_runs() {
                    if let Err(error) = buffer.edit(
                        [(text_added_offset..text_added_offset)],
                        Text::new(styled_text.text(), Some(styled_text.text_style())),
                        ctx,
                    ) {
                        report_error!(error.context("error inserting text"));
                    };
                    text_added_offset += styled_text.text().chars().count();
                }
            });

            let edits = self.buffer(ctx).edits_since(version).collect::<Vec<_>>();
            self.display_map_handle()
                .update(ctx, move |display_map, ctx| {
                    display_map.apply_edits(&edits, ctx).unwrap();
                });

            let buffer = self.buffer(ctx);
            let restored_selection = snapshot
            .selections()
            .iter()
            .filter_map(|range| {
                // Handle the error to convert saved selection offsets to editor anchors gracefully
                // as the restored buffer state might not match exactly to the saved snapshot.
                let Some(end) = buffer.anchor_before(range.end).ok() else {
                    report_error!(
                        "error restoring snapshot: selection end is past text max range",
                        extra: { "selection_end" => %range.end, "max_range" => %text_added_offset }
                    );
                    return None;
                };
                let Some(start) = buffer.anchor_before(range.start).ok() else {
                    report_error!(
                        "error restoring snapshot: selection start is past text max range",
                        extra: { "selection_start" => %range.start, "max_range" => %text_added_offset }
                    );
                    return None;
                };

                let mut selection = LocalSelection {
                    selection: Selection::single_cursor(end),
                    clamp_direction: Default::default(),
                    goal_start_column: None,
                    goal_end_column: None,
                };

                selection.set_head(buffer, start);
                Some(selection)
            })
            .collect_vec();

            let new_selections =
                if let Ok(restored_selection) = Vec1::try_from_vec(restored_selection) {
                    restored_selection
                } else {
                    // Make sure there is at least one root selection if the
                    // restored selection is empty.
                    vec1![LocalSelection {
                        selection: Selection::single_cursor(Anchor::Start),
                        clamp_direction: Default::default(),
                        goal_start_column: None,
                        goal_end_column: None,
                    }]
                };
            self.change_selections(new_selections, ctx);
        }

        pub fn selection_insertion_index(&self, start: &Anchor, app: &AppContext) -> usize {
            self.buffer(app)
                .local_selections()
                .selection_insertion_index(start, self.buffer(app))
        }

        /// Helper method for moving the cursor within the input box / editor.
        /// @param keep_selection - If true, the movement will also select the text spanned by the
        /// movement.
        /// @param func - Closure which gets buffer & selection and returns the location to which the
        /// cursor should be moved.
        /// @param ctx
        pub fn move_cursor<F, C>(
            &mut self,
            keep_selection: bool,
            func: F,
            ctx: &mut ModelContext<Self>,
        ) where
            C: ToCharOffset,
            F: Fn(&Buffer, &mut LocalSelection) -> C,
        {
            let mut new_selections = self.selections(ctx).clone();
            let buffer = self.buffer(ctx);
            for selection in new_selections.iter_mut() {
                let point = func(buffer, selection);
                let cursor = buffer.anchor_before(point).unwrap();

                if keep_selection {
                    selection.set_head(buffer, cursor);
                } else {
                    selection.set_selection(Selection::single_cursor(cursor));
                }
                selection.goal_start_column = None;
                selection.goal_end_column = None;
            }
            self.change_selections(new_selections, ctx);
        }

        pub fn cursor_line_start(&mut self, keep_selection: bool, ctx: &mut ModelContext<Self>) {
            self.move_cursor(
                keep_selection,
                |buffer, selection| {
                    let start = selection.head().to_point(buffer).unwrap();
                    Point::new(start.row, 0)
                },
                ctx,
            );
        }

        pub fn cursor_line_end(&mut self, keep_selection: bool, ctx: &mut ModelContext<Self>) {
            self.move_cursor(
                keep_selection,
                |buffer, selection| {
                    let end = selection.end().to_point(buffer).unwrap();
                    Point::new(end.row, buffer.line_len(end.row).unwrap())
                },
                ctx,
            );
        }

        pub fn move_to_buffer_end(&mut self, keep_selection: bool, ctx: &mut ModelContext<Self>) {
            self.move_cursor(keep_selection, |buffer, _| buffer.max_point(), ctx);
        }

        /// Clear any selected regions, leaving the cursor at the end of the first selection
        pub fn clear_selections(&mut self, ctx: &mut ModelContext<Self>) {
            if !self.selections(ctx).is_empty() {
                let buffer = self.buffer(ctx);
                let mut first_selection = self.first_selection(ctx).clone();
                first_selection.set_head(buffer, first_selection.tail().clone());
                self.change_selections(vec1![first_selection], ctx);
            }
        }

        pub fn clear_buffer(&mut self, ctx: &mut ModelContext<Self>) {
            self.buffer_handle().update(ctx, |buffer, ctx| {
                if let Err(error) = buffer.edit(Some(0.into()..buffer.len()), "", ctx) {
                    report_error!(error.context("error clearing text"));
                };
            });
            self.clear_selections(ctx);
        }

        /// Replaces the first n characters in the buffer with the given text, while
        /// keeping any existing selections untouched (but consolidating in case
        /// the existing selections are no longer valid after the replacement).
        pub fn replace_first_n_characters(
            &mut self,
            n: CharOffset,
            text: &str,
            ctx: &mut ModelContext<Self>,
        ) {
            self.buffer_handle().update(ctx, |buffer, ctx| {
                if let Err(error) = buffer.edit(Some(0.into()..n), text, ctx) {
                    report_error!(error.context("error replacing first n chars"));
                };
            });
        }

        /// Replaces the last `n` characters in the buffer with the given text, while
        /// keeping any existing selections untouched (but consolidating in case
        /// the existing selections are no longer valid after the replacement).
        pub fn replace_last_n_characters(
            &mut self,
            n: CharOffset,
            text: &str,
            ctx: &mut ModelContext<Self>,
        ) {
            self.buffer_handle().update(ctx, |buffer, ctx| {
                let len = buffer.len();
                let start = len.saturating_sub(&n);
                if let Err(error) = buffer.edit(Some(start..len), text, ctx) {
                    report_error!(error.context("error replacing last n chars"));
                };
            });
        }

        pub fn reset_selections_to_point(&mut self, point: &Point, ctx: &mut ModelContext<Self>) {
            let buffer = self.buffer(ctx);
            let cursor = buffer
                .anchor_before(point.to_char_offset(buffer).unwrap())
                .unwrap();

            let mut first_selection = self.first_selection(ctx).clone();
            first_selection.set_selection(Selection::single_cursor(cursor));
            self.change_selections(vec1![first_selection], ctx);
        }

        /// Selects ranges by `CharOffset`s. Note if the selections specified in `ranges` are empty, the
        /// selections are not set since there must be at least one selection.
        pub fn select_ranges_by_offset<T, G>(
            &mut self,
            ranges: T,
            ctx: &mut ModelContext<Self>,
        ) -> anyhow::Result<()>
        where
            T: IntoIterator<Item = Range<G>>,
            G: ToCharOffset,
        {
            let buffer = self.buffer(ctx);
            let new_selections: Vec<_> = ranges
                .into_iter()
                .flat_map(|range| {
                    let start = range.start.to_char_offset(buffer).ok()?;
                    let end = range.end.to_char_offset(buffer).ok()?;

                    Some(LocalSelection {
                        selection: Selection {
                            start: buffer.anchor_after(start).ok()?,
                            end: buffer.anchor_before(end).ok()?,
                            reversed: false,
                        },
                        clamp_direction: Default::default(),
                        goal_start_column: None,
                        goal_end_column: None,
                    })
                })
                .sorted_by(|a, b| {
                    a.start()
                        .cmp(b.start(), buffer)
                        .expect("anchors should be comparable")
                })
                .collect();

            // Only set the selections if the new selections are not empty.
            if let Ok(new_selections) = Vec1::try_from_vec(new_selections) {
                self.change_selections(new_selections, ctx);
            }

            Ok(())
        }

        pub fn select_ranges_by_display_point<T>(
            &mut self,
            ranges: T,
            ctx: &mut ModelContext<Self>,
        ) -> anyhow::Result<()>
        where
            T: IntoIterator<Item = Range<DisplayPoint>>,
        {
            let buffer = self.buffer(ctx);
            let map = self.display_map(ctx);
            let mut new_selections = Vec::new();
            for range in ranges {
                new_selections.push(LocalSelection {
                    selection: Selection {
                        start: map.anchor_after(range.start, Bias::Left, ctx)?,
                        end: map.anchor_before(range.end, Bias::Left, ctx)?,
                        reversed: false,
                    },
                    clamp_direction: Default::default(),
                    goal_start_column: None,
                    goal_end_column: None,
                });
            }
            new_selections.sort_unstable_by(|a, b| a.start().cmp(b.start(), buffer).unwrap());

            if let Ok(new_selections) = Vec1::try_from_vec(new_selections) {
                self.change_selections(new_selections, ctx);
            }

            Ok(())
        }

        pub fn select_word_right(&mut self, ctx: &mut ModelContext<Self>) {
            let buffer = self.buffer(ctx);
            let mut new_selections = self.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                let start_position = selection.start().to_point(buffer).unwrap();
                if let Ok(mut word_ends) = buffer.word_ends_from_offset_exclusive(start_position) {
                    let word_end = word_ends.next().unwrap_or(start_position);
                    let cursor = buffer.anchor_before(word_end).unwrap();
                    selection.set_end(cursor);
                }
            }
            self.change_selections(new_selections, ctx);
        }

        pub fn select_word_left(&mut self, ctx: &mut ModelContext<Self>) {
            let buffer = self.buffer(ctx);
            let mut new_selections = self.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                let start_position = selection.start().to_point(buffer).unwrap();
                if let Ok(mut word_starts) =
                    buffer.word_starts_backward_from_offset_exclusive(start_position)
                {
                    let word_start = word_starts.next().unwrap_or(start_position);
                    let cursor = buffer.anchor_before(word_start).unwrap();
                    selection.set_start(cursor);
                }
            }
            self.change_selections(new_selections, ctx);
        }

        /// Helper method to get an anchor at the start of the word closest to the given position
        pub fn get_word_start(
            &mut self,
            position: DisplayPoint,
            policy: WordBoundariesPolicy,
            ctx: &ModelContext<Self>,
        ) -> Anchor {
            let app = ctx;
            let buffer = self.buffer(ctx);
            let map = self.display_map(ctx);
            let position = position
                .to_buffer_point(map, Bias::Left, ctx)
                .expect("Should be able to get point");

            // Get the previous word start, including the current position if applicable
            let word_start = buffer
                .word_starts_backward_from_offset_inclusive(position)
                .ok()
                .and_then(|word_starts| word_starts.with_policy(policy).next())
                .unwrap_or(position);

            map.anchor_before(
                word_start
                    .to_display_point(map, app)
                    .expect("Should be able to get start point of word"),
                Bias::Left,
                app,
            )
            .expect("Should be able to get anchor")
        }

        /// Helper method to get an anchor at the end of the word closest to the given position
        pub fn get_word_end(
            &mut self,
            position: DisplayPoint,
            policy: WordBoundariesPolicy,
            ctx: &ModelContext<Self>,
        ) -> Anchor {
            let app = ctx;
            let buffer = self.buffer(ctx);
            let map = self.display_map(ctx);
            let position = position
                .to_buffer_point(map, Bias::Left, ctx)
                .expect("Should be able to get point");

            // Get the next word ending, including the current position if applicable
            let word_end = match buffer
                .word_ends_from_offset_inclusive(position)
                .ok()
                .and_then(|word_ends| word_ends.with_policy(policy).next())
            {
                Some(end) => end,
                None => position,
            };

            map.anchor_before(
                word_end
                    .to_display_point(map, app)
                    .expect("Should be able to get start point of word"),
                Bias::Left,
                app,
            )
            .expect("Should be able to get anchor")
        }

        pub fn max_point(&self, app: &AppContext) -> DisplayPoint {
            self.display_map(app).max_point(app)
        }

        pub fn backspace(&mut self, ctx: &mut ModelContext<Self>) {
            // Only select left for empty selections
            let buffer = self.buffer(ctx);
            let mut new_selections = self.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                if selection.start().to_point(buffer).unwrap()
                    == selection.end().to_point(buffer).unwrap()
                {
                    let head = selection
                        .head()
                        .to_char_offset(buffer)
                        .expect("Selection head should exist");
                    let cursor = buffer
                        .anchor_before(
                            head.saturating_sub(&1.into())
                                .to_point(buffer)
                                .expect("Delete offset should exist"),
                        )
                        .expect("Offset should be in range");
                    selection.set_head(buffer, cursor);
                }
            }

            ctx.emit_a11y_content(AccessibilityContent::new(
                self.selected_text(ctx),
                ", deleted",
                WarpA11yRole::UserAction,
            ));
            self.change_selections(new_selections, ctx);
            self.insert("", None, ctx);
        }

        pub fn delete(&mut self, ctx: &mut ModelContext<Self>) {
            let buffer = self.buffer(ctx);
            let mut new_selections = self.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                if selection.start().to_point(buffer).unwrap()
                    == selection.end().to_point(buffer).unwrap()
                {
                    let head = selection.head().to_char_offset(buffer).unwrap();
                    if let Ok(point) = (head + 1).to_point(buffer) {
                        let cursor = buffer.anchor_before(point).unwrap();
                        selection.set_head(buffer, cursor);
                    }
                }
            }

            ctx.emit_a11y_content(AccessibilityContent::new(
                self.selected_text(ctx),
                ", deleted",
                WarpA11yRole::UserAction,
            ));
            self.change_selections(new_selections, ctx);
            self.insert("", None, ctx);
        }

        // Deletes the character before and after the cursor.
        pub fn remove_before_and_after_cursor(&mut self, ctx: &mut ModelContext<Self>) {
            let buffer = self.buffer(ctx);
            let mut new_selections = self.selections(ctx).clone();
            for selection in new_selections.iter_mut() {
                if selection.start().to_point(buffer).unwrap()
                    == selection.end().to_point(buffer).unwrap()
                {
                    let head = selection.head().to_char_offset(buffer).unwrap();

                    if selection.reversed() {
                        selection.set_end(
                            buffer
                                .anchor_before(
                                    head.saturating_sub(&1.into()).to_point(buffer).unwrap(),
                                )
                                .expect("Selection offset should be in range"),
                        );
                    } else {
                        selection.set_start(
                            buffer
                                .anchor_before(
                                    head.saturating_sub(&1.into()).to_point(buffer).unwrap(),
                                )
                                .expect("Selection offset should be in range"),
                        );
                    }

                    if let Ok(point) = (head + 1).to_point(buffer) {
                        let cursor = buffer.anchor_before(point).unwrap();
                        selection.set_head(buffer, cursor);
                    }
                }
            }
            self.consecutive_autocomplete_insertion_edits_counter -= 1;
            self.change_selections(new_selections, ctx);
            self.insert("", None, ctx);
        }

        pub fn shell_cut(&mut self, ctx: &mut ModelContext<Self>) {
            let text = self.selected_text(ctx);
            if !text.is_empty() {
                ctx.emit(EditorModelEvent::ShellCut(text));
                self.backspace(ctx);
            }
        }

        /// Returns all local selections that intersect the provided range.
        pub fn local_selections_intersecting_range<'a>(
            &'a self,
            range: Range<DisplayPoint>,
            app: &'a AppContext,
        ) -> impl 'a + Iterator<Item = (&'a LocalSelection, Range<DisplayPoint>)> {
            let map = self.display_map(app);
            self.buffer(app)
                .local_selections()
                .selections_intersecting_range(range.clone(), map, app)
        }

        /// Returns drawable selections that intersect the provided range.
        pub fn all_drawable_selections_intersecting_range<'a>(
            &'a self,
            range: Range<DisplayPoint>,
            app: &'a AppContext,
        ) -> impl 'a + Iterator<Item = DrawableSelection> {
            let map = self.display_map(app);
            self.buffer(app)
                .local_selections()
                .drawable_selections_intersecting_range(range, map, app)
        }

        pub fn pending_selection<'a, C: ModelAsRef>(
            &'a self,
            ctx: &'a C,
        ) -> Option<&'a LocalPendingSelection> {
            self.buffer(ctx).local_selections().pending.as_ref()
        }

        pub fn selections<'a, C: ModelAsRef>(&'a self, ctx: &'a C) -> &'a Vec1<LocalSelection> {
            &self.buffer(ctx).local_selections().selections
        }

        fn is_ephemeral(&self) -> bool {
            self.buffer_and_display_map.ephemeral.is_some()
        }

        /// We only expose an immutable reference to the [`Buffer`]
        /// so that callers can query anchor positions and such.
        /// We do _not_ want to expose an arbitrary model handle
        /// because that can otherwise be `update`d.
        pub fn buffer<'a, C: ModelAsRef>(&self, ctx: &'a C) -> &'a Buffer {
            self.buffer_handle().as_ref(ctx)
        }

        /// We only expose an immutable reference to the [`Buffer`]
        /// so that callers can query visible ranges and such.
        /// We do _not_ want to expose an arbitrary model handle
        /// because that can otherwise be `update`d.
        pub fn display_map<'a, C: ModelAsRef>(&self, ctx: &'a C) -> &'a DisplayMap {
            self.display_map_handle().as_ref(ctx)
        }

        fn buffer_handle(&self) -> &ModelHandle<Buffer> {
            self.buffer_and_display_map.buffer_handle()
        }

        fn display_map_handle(&self) -> &ModelHandle<DisplayMap> {
            self.buffer_and_display_map.display_map_handle()
        }

        pub fn buffer_text<C: ModelAsRef>(&self, ctx: &C) -> String {
            self.buffer(ctx).text()
        }

        /// The length of the buffer as a [`CharOffset`].
        pub fn buffer_len<C: ModelAsRef>(&self, ctx: &C) -> CharOffset {
            self.buffer(ctx).len()
        }

        pub fn last_buffer_text(&self) -> &str {
            self.last_buffer_text.as_str()
        }

        pub fn last_action<C: ModelAsRef>(&self, ctx: &C) -> Option<PlainTextEditorViewAction> {
            self.buffer(ctx).last_action()
        }

        pub fn first_selection<'a, C: ModelAsRef>(&'a self, ctx: &'a C) -> &'a LocalSelection {
            self.buffer(ctx).local_selections().first()
        }

        pub fn last_selection<'a, C: ModelAsRef>(&'a self, ctx: &'a C) -> &'a LocalSelection {
            self.buffer(ctx).local_selections().last()
        }

        pub fn is_single_cursor_only<C: ModelAsRef>(&self, ctx: &C) -> bool {
            self.selections(ctx).len() == 1
                && self.selections(ctx)[0].is_cursor_only(self.buffer(ctx))
        }

        pub fn is_selecting(&self, app: &AppContext) -> bool {
            self.pending_selection(app).is_some()
        }

        pub fn line_len(&self, display_row: u32, app: &AppContext) -> anyhow::Result<u32> {
            self.display_map(app).line_len(display_row, app)
        }

        pub fn chars_preceding_selections<'a, C: ModelAsRef>(
            &'a self,
            app: &'a C,
        ) -> impl Iterator<Item = Chars<'a>> + 'a {
            let buffer = self.buffer(app);
            self.selections(app)
                .iter()
                .filter_map(|selection| buffer.chars_at(selection.start()).ok())
        }

        pub fn any_selections_span_entire_buffer<C: ModelAsRef>(&self, app: &C) -> bool {
            let buffer = self.buffer(app);
            self.selections(app)
                .iter()
                .any(|selection| selection.spans_entire_buffer(buffer))
        }
    }

    /// The private interface.
    impl EditorModel {
        fn handle_buffer_event(
            &mut self,
            _: ModelHandle<Buffer>,
            event: &buffer::Event,
            ctx: &mut ModelContext<Self>,
        ) {
            match event {
                buffer::Event::Edited { edit_origin, .. } => ctx.emit(EditorModelEvent::Edited {
                    edit_origin: *edit_origin,
                }),
                buffer::Event::SelectionsChanged => ctx.emit(EditorModelEvent::SelectionsChanged),
                buffer::Event::StylesUpdated => ctx.emit(EditorModelEvent::StylesUpdated),
            }
        }

        fn handle_display_map_event(
            &mut self,
            _: ModelHandle<DisplayMap>,
            event: &display_map::Event,
            ctx: &mut ModelContext<Self>,
        ) {
            match event {
                display_map::Event::Folded | display_map::Event::Unfolded => {
                    ctx.emit(EditorModelEvent::DisplayMapUpdated)
                }
            }
        }

        fn selection_to_char_offset_ranges(
            &self,
            ctx: &mut ModelContext<Self>,
        ) -> Vec1<Range<CharOffset>> {
            self.selections(ctx).clone().mapped(|selection| {
                let start = selection.start().to_char_offset(self.buffer(ctx)).unwrap();
                let end = selection.end().to_char_offset(self.buffer(ctx)).unwrap();
                start..end
            })
        }

        // Insert characters around the current selections. For example, if the selection is "abc"
        // and the before_cursor_text is "(" and after_cursor_text is ")", the resulting
        // buffer will be "(abc)".
        fn insert_characters_wrap_around_selection(
            &mut self,
            before_cursor_text: &str,
            after_cursor_text: &str,
            ctx: &mut ModelContext<Self>,
        ) {
            let offset_ranges = self.selection_to_char_offset_ranges(ctx);
            let before_cursor_char_len = before_cursor_text.chars().count();
            let after_cursor_char_len = after_cursor_text.chars().count();

            self.buffer_handle().update(ctx, |buffer, ctx| {
                // Inserting the user-typed characters.
                if let Err(error) = buffer.edit(
                    offset_ranges
                        .iter()
                        .map(|range| range.start..range.start)
                        .clone(),
                    before_cursor_text,
                    ctx,
                ) {
                    report_error!(error.context("error inserting text"));
                };

                // Inserting the autocompleted characters. Because the buffer has
                // changed due to the insertion of user-typed characters, we need to
                // shift the insertion range by its index here.
                if let Err(error) = buffer.edit(
                    offset_ranges
                        .iter()
                        .enumerate()
                        .map(|(idx, range)| range.end + idx + 1..range.end + idx + 1)
                        .clone(),
                    after_cursor_text,
                    ctx,
                ) {
                    report_error!(error.context("error inserting text"));
                };
            });

            let buffer = self.buffer(ctx);
            let mut delta = before_cursor_char_len;

            let new_selections = offset_ranges.mapped(|range| {
                let range_start = range.start;
                let range_end = range.end;
                let end = buffer.anchor_before(range_end + delta).unwrap();
                let start = buffer.anchor_before(range_start + delta).unwrap();

                delta += before_cursor_char_len + after_cursor_char_len;
                LocalSelection {
                    selection: Selection {
                        start,
                        end,
                        reversed: false,
                    },
                    clamp_direction: Default::default(),
                    goal_start_column: None,
                    goal_end_column: None,
                }
            });
            self.change_selections(new_selections, ctx);
        }

        fn no_cursor_only_selections(&self, ctx: &mut ModelContext<Self>) -> bool {
            self.selections(ctx)
                .iter()
                .all(|selection| !selection.is_cursor_only(self.buffer(ctx)))
        }

        fn all_cursors_next_character_matches_char(
            &self,
            character: char,
            ctx: &mut ModelContext<Self>,
        ) -> bool {
            let buffer = self.buffer(ctx);

            self.selections(ctx).iter().all(|selection| {
                let position = selection
                    .start()
                    .to_point(buffer)
                    .expect("Start of selection should exist");

                buffer
                    .chars_at(position)
                    .unwrap()
                    .next()
                    .map(|right_char| right_char == character)
                    .unwrap_or(false)
            })
        }

        /// Determines if adding the given string `text` to the buffer (which replaces all selections)
        /// would exceed the maximum buffer length.
        fn text_insertion_would_exceed_max_buffer_len(&self, text: &str, app: &AppContext) -> bool {
            let Some(max_buffer_len) = self.max_buffer_len else {
                return false;
            };
            let text_len = text.chars().count();
            let selections = self.selected_text_strings(app);
            // `diff` represents the difference in length that inserting `text` into each
            // selection would result in.
            let diff = selections.iter().fold(0, |running_diff, curr| {
                let diff_from_selection = text_len as isize - curr.chars().count() as isize;
                running_diff + diff_from_selection
            });
            let buffer = self.buffer(app);
            buffer.len().as_usize() as isize + diff > max_buffer_len as isize
        }

        fn text_is_invalid_input_type(&self, text: &str) -> bool {
            match self.valid_input_type {
                ValidInputType::All => false,
                ValidInputType::PositiveInteger => text.parse::<u16>().is_err() && !text.is_empty(),
                ValidInputType::NoSpaces => text.contains(char::is_whitespace),
            }
        }
    }

    mod buffer {

        use std::cmp::{self};
        use std::collections::{BTreeSet, HashMap};
        use std::iter::{self, Iterator};
        use std::ops::{AddAssign, Range};
        use std::str;

        use anyhow::{Result, anyhow};
        use lazy_static::lazy_static;
        use serde::{Deserialize, Serialize};
        use string_offset::{ByteOffset, CharOffset};
        use sum_tree::{self, Cursor, FilterCursor, SeekBias, SumTree};
        use time::{Global, Lamport};
        use undo::{LocalUndoStack, UndoHistory};
        use vec1::{Vec1, vec1};
        use warpui::color::ColorU;
        use warpui::text::point::Point;
        use warpui::text::{BufferIndex, TextBuffer};
        use warpui::text_layout::TextStyle;
        use warpui::{Entity, ModelContext};
        /// The public interfaces that we expose to the model.
        /// This should be a very limited set of APIs and should
        /// not expose the internal details of the buffer.
        pub use {
            anchor::{Anchor, AnchorBias, AnchorRangeExt},
            subword_boundaries::SubwordBoundaries,
            text::{Text, TextSummary},
            time::{LamportValue, ReplicaId},
        };

        use super::selections::{AsSelection, LocalSelections, MarkedTextState, Selection};
        use super::{EditorSnapshot, LocalSelection};
        use crate::editor::PlainTextEditorViewAction;

        pub struct Buffer {
            /// The latest observed lamport timestamp.
            /// When we mutate the buffer ourselves (edit / undo / selection change),
            /// we tick the clock.
            ///
            /// The replica ID of the clock is the replica ID of this peer.
            lamport_clock: Lamport,

            /// The set of edits we've observed from all peers in the system.
            ///
            /// Selection changes are not observed in the version vector;
            /// while selections depend on edits, the converse is not true.
            versions: Global,

            /// The main data structure of the buffer.
            /// Responsible for storing the edit history
            /// in a tree for fast lookup and insertion.
            fragments: SumTree<Fragment>,

            /// A mapping between an insertion ID and the set of
            /// edits that were made relative to that ID.
            insertion_splits: HashMap<Lamport, SumTree<InsertionSplit>>,

            /// The current batch of changes, if any.
            /// All CRDT-compliant buffer changes must be batched.
            /// There can only be one batch active at a time.
            ///
            /// Today, edits to the buffer are _not_ sent in a batched form (e.g.
            /// 1 CRDT operation for N edits) to peers.
            batch_state: BatchState,

            /// The selection state for this peer.
            local_selections: LocalSelections,

            /// The undo stack for local edits.
            local_undo_stack: LocalUndoStack,

            /// The undo history for local and remote edits.
            undo_history: UndoHistory,
        }

        /// Whether changes to the buffer are being batched
        /// and the nature of the batch (if any).
        #[derive(Clone)]
        enum BatchState {
            /// There is no on-going batch.
            Inactive,
            /// There is an active batch but it only supports selection changes.
            SelectionChangesOnly {
                /// The set of selections before the batch started.
                selections_before_batch: LocalSelections,
            },
            /// There is an active batch and it supports both edits and selection changes.
            EditsAndSelectionChanges {
                /// The origin of the batched edit.
                edit_origin: EditOrigin,
                /// The action associated to the batched edit.
                action: PlainTextEditorViewAction,
                /// The set of local edit operations for the current batch.
                edits: Vec<EditOperation>,
                /// The number of [`Self::edits`] already recorded on the undo stack.
                /// This relies on [`edits`] being append-only.
                /// We support an API ([`Buffer::record_edits`]) to record the intermediate edits
                /// to the undo stack so we keep track of which are already recorded to avoid double-recording.
                num_edits_already_recorded: usize,
                /// The set of selections before the batch started.
                selections_before_batch: LocalSelections,
                /// The version vector before the batch started.
                since: Global,
                /// Whether or not we should skip recording the edits on the
                /// undo stack when finishing the batch.
                skip_undo: bool,
            },
            /// There is an active batch for an undo / redo operation.
            UndoRedo {
                /// The version vector before the batch started.
                since: Global,
                undo_op: Option<UndoOperation>,
            },
        }

        impl BatchState {
            fn is_batching(&self) -> bool {
                !matches!(self, Self::Inactive)
            }

            fn extend_edits(&mut self, new_edits: Vec<EditOperation>) {
                debug_assert!(matches!(self, Self::EditsAndSelectionChanges { .. }));

                match self {
                    Self::Inactive => {
                        log::warn!("Tried to extend batch when we weren't already batching");
                    }
                    Self::SelectionChangesOnly { .. } => {
                        log::warn!("Tried to add edits to selection-only batch");
                    }
                    Self::UndoRedo { .. } => {
                        log::warn!("Tried to add edits to undo/redo batch");
                    }
                    Self::EditsAndSelectionChanges { edits, .. } => {
                        edits.extend(new_edits);
                    }
                }
            }

            /// Ensures that we are in an appropriate batching state when attempting
            /// to make a selection change to the buffer.
            ///
            /// Panics in debug mode, if not.
            fn attempt_selection_change(&self) -> Result<()> {
                if !self.is_batching() {
                    debug_assert!(false, "Cannot change selections outside of a batch");
                    anyhow::bail!("Cannot change selections outside of a batch");
                }
                Ok(())
            }

            /// Ensures that we are in an appropriate batching state when attempting
            /// to make an edit to the buffer.
            ///
            /// Panics in debug mode, if not.
            fn attempt_edit(&self) -> Result<()> {
                let res = match self {
                    Self::Inactive => Err(anyhow!("Cannot edit outside of a batch")),
                    Self::SelectionChangesOnly { .. } | Self::UndoRedo { .. } => {
                        Err(anyhow!("Batch does not support edits"))
                    }
                    Self::EditsAndSelectionChanges { .. } => Ok(()),
                };
                if let Err(e) = &res {
                    debug_assert!(false, "{e}");
                }
                res
            }

            /// Ensures that we are in an appropriate batching state when attempting
            /// to perform an undo / redo.
            ///
            /// Panics in debug mode, if not.
            fn attempt_undo(&self) -> Result<()> {
                let res = match self {
                    Self::Inactive => Err(anyhow!("Cannot undo outside of a batch")),
                    Self::SelectionChangesOnly { .. } | Self::EditsAndSelectionChanges { .. } => {
                        Err(anyhow!("Batch does not support undo / redo"))
                    }
                    Self::UndoRedo { .. } => Ok(()),
                };
                if let Err(e) = &res {
                    debug_assert!(false, "{e}");
                }
                res
            }

            fn set_undo_op(&mut self, op: UndoOperation) {
                match self {
                    Self::Inactive => {
                        log::warn!("Tried to add undo op when we weren't already batching");
                    }
                    Self::SelectionChangesOnly { .. } => {
                        log::warn!("Tried to add undo op during selections-only batch");
                    }
                    Self::UndoRedo { undo_op, .. } => {
                        *undo_op = Some(op);
                    }
                    Self::EditsAndSelectionChanges { .. } => {
                        log::warn!("Tried to add undo op during edits batch");
                    }
                }
            }
        }

        /// What initiated the change in the buffer.
        #[derive(PartialEq, Eq, Clone, Copy, Debug)]
        pub enum EditOrigin {
            /// The user typed the change by entering characters.
            UserTyped,

            /// The user didn't type a character but the user did initiate the change (e.g. backspace, paste, etc.).
            UserInitiated,

            /// The user didn't initiate this change. For example, an unsolicited update
            /// from the server replaces some client-side buffer.
            SystemEdit,
        }

        impl EditOrigin {
            pub fn is_user(&self) -> bool {
                matches!(self, EditOrigin::UserTyped) || matches!(self, EditOrigin::UserInitiated)
            }
        }

        #[derive(Clone)]
        pub struct CharsWithStyle<'a> {
            fragments_cursor: Cursor<'a, Fragment, CharOffset, CharOffset>,
            fragment_chars: str::Chars<'a>,
            fragment_text_style: TextStyle,
            fragment_offset: CharOffset,
            reversed: bool,
            undo_history: &'a UndoHistory,
        }

        pub struct Chars<'a>(CharsWithStyle<'a>);

        /// StyleOperation is used for storing data pertaining to a single operation
        /// that can be performed on a particular TextStyle property e.g. foreground color.
        #[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
        pub enum StyleOperation {
            // None is the default case where we don't change the existing style,
            // if any exists.
            None,
            // Clear specifies explicitly that we wish to clear any existing style,
            // if it exists.
            Clear,
            // Style specifies that we wish to add/overwrite the existing style
            // with the new style (includes specifying a color).
            Style { color: ColorU },
        }

        impl StyleOperation {
            /// Get resulting color from applying the current StyleOperation onto
            /// some current color.
            fn get_result_color(self, current_color: Option<ColorU>) -> Option<ColorU> {
                match self {
                    Self::Style { color } => Some(color),
                    Self::Clear => None,
                    Self::None => current_color,
                }
            }
        }

        /// TextStyleOperation defines a single operation for updating
        /// text styles - it can update each property of the TextStyle e.g. foreground
        /// color, background color, etc.
        #[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
        pub struct TextStyleOperation {
            foreground_color: StyleOperation,
            syntax_color: StyleOperation,
            background_color: StyleOperation,
            error_underline_color: StyleOperation,
        }

        impl Default for TextStyleOperation {
            fn default() -> Self {
                TextStyleOperation::new()
            }
        }

        impl TextStyleOperation {
            pub fn new() -> Self {
                TextStyleOperation {
                    foreground_color: StyleOperation::None,
                    syntax_color: StyleOperation::None,
                    background_color: StyleOperation::None,
                    error_underline_color: StyleOperation::None,
                }
            }

            /// Applies a certain TextStyleOperation onto a TextStyle and returns
            /// the resulting new TextStyle.
            ///
            /// # Arguments
            ///
            /// * `text_style` - The TextStyle we want to apply the TextStyleOperation on.
            /// * `text_style_operation` - The TextStyleOperation to apply.
            ///
            /// # Example
            /// ```
            /// use warpui::color::ColorU;
            /// use warpui::text_layout::TextStyle;
            /// use ml_editor::TextStyleOperation;
            /// TextStyleOperation::apply_text_style_operation(
            ///     TextStyle::default(),
            ///     TextStyleOperation::default().set_error_underline_color(ColorU::black()),
            /// );
            /// ```
            ///
            pub fn apply_text_style_operation(
                mut text_style: TextStyle,
                text_style_operation: TextStyleOperation,
            ) -> TextStyle {
                text_style.foreground_color = text_style_operation
                    .foreground_color
                    .get_result_color(text_style.foreground_color);
                text_style.syntax_color = text_style_operation
                    .syntax_color
                    .get_result_color(text_style.syntax_color);
                text_style.background_color = text_style_operation
                    .background_color
                    .get_result_color(text_style.background_color);
                text_style.error_underline_color = text_style_operation
                    .error_underline_color
                    .get_result_color(text_style.error_underline_color);
                text_style
            }

            pub fn set_foreground_color(mut self, foreground_color: ColorU) -> Self {
                self.foreground_color = StyleOperation::Style {
                    color: foreground_color,
                };
                self
            }

            pub fn clear_foreground_color(mut self) -> Self {
                self.foreground_color = StyleOperation::Clear;
                self
            }

            pub fn set_syntax_color(mut self, syntax_color: ColorU) -> Self {
                self.syntax_color = StyleOperation::Style {
                    color: syntax_color,
                };
                self
            }

            pub fn clear_syntax_color(mut self) -> Self {
                self.syntax_color = StyleOperation::Clear;
                self
            }

            pub fn set_background_color(mut self, background_color: ColorU) -> Self {
                self.background_color = StyleOperation::Style {
                    color: background_color,
                };
                self
            }

            pub fn clear_background_color(mut self) -> Self {
                self.background_color = StyleOperation::Clear;
                self
            }

            pub fn set_error_underline_color(mut self, error_underline_color: ColorU) -> Self {
                self.error_underline_color = StyleOperation::Style {
                    color: error_underline_color,
                };
                self
            }

            pub fn clear_error_underline_color(mut self) -> Self {
                self.error_underline_color = StyleOperation::Clear;
                self
            }

            pub fn clear_all() -> Self {
                Self {
                    foreground_color: StyleOperation::Clear,
                    syntax_color: StyleOperation::Clear,
                    background_color: StyleOperation::Clear,
                    error_underline_color: StyleOperation::Clear,
                }
            }

            /// Set the text style to have no decorations (underlines or syntax color).
            pub fn clear_decorations(self) -> Self {
                self.clear_error_underline_color().clear_syntax_color()
            }
        }

        pub struct TextStyleRuns<'a> {
            fragments_cursor: Cursor<'a, Fragment, ByteOffset, ByteOffset>,
            byte_index_start: ByteOffset,
            byte_index_end: ByteOffset,
            undo_history: &'a UndoHistory,
        }

        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct TextRun {
            text: String,
            text_style: TextStyle,
            // Range of the text in the overall buffer.
            range: Range<ByteOffset>,
        }

        impl TextRun {
            pub fn new(text: String, text_style: TextStyle, range: Range<ByteOffset>) -> Self {
                TextRun {
                    text,
                    text_style,
                    range,
                }
            }

            pub fn text_from_text_runs(text_runs: &[TextRun]) -> String {
                text_runs
                    .iter()
                    .map(|text_run| text_run.text().to_string())
                    .collect::<Vec<String>>()
                    .join("")
            }
        }

        impl TextRun {
            pub fn text(&self) -> &str {
                &self.text
            }

            pub fn text_style(&self) -> TextStyle {
                self.text_style
            }

            pub fn byte_range(&self) -> &Range<ByteOffset> {
                &self.range
            }
        }

        #[derive(Clone)]
        pub struct StylizedChar {
            char: char,
            style: TextStyle,
        }

        impl StylizedChar {
            pub fn new(char: char, style: TextStyle) -> Self {
                StylizedChar { char, style }
            }

            pub fn char(&self) -> char {
                self.char
            }

            pub fn style(&self) -> TextStyle {
                self.style
            }
        }

        impl From<StylizedChar> for char {
            fn from(stylized_char: StylizedChar) -> Self {
                stylized_char.char
            }
        }

        struct Edits<'a, F: Fn(&FragmentSummary) -> bool> {
            cursor: FilterCursor<'a, F, Fragment, CharOffset>,
            since: Global,
            delta: isize,
            undo_history: &'a UndoHistory,
        }

        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct Edit {
            pub old_range: Range<CharOffset>,
            pub new_range: Range<CharOffset>,
        }

        impl Edit {
            pub fn delta(&self) -> isize {
                (self.new_range.end.as_usize() - self.new_range.start.as_usize()) as isize
                    - (self.old_range.end.as_usize() - self.old_range.start.as_usize()) as isize
            }

            pub fn old_extent(&self) -> CharOffset {
                self.old_range.end - self.old_range.start
            }
        }

        #[derive(Clone, Eq, PartialEq, Debug)]
        pub struct Insertion {
            id: Lamport,
            parent_id: Lamport,
            offset_in_parent: CharOffset,
            text: Text,
        }

        #[derive(Eq, PartialEq, Clone, Debug)]
        struct Fragment {
            /// The id of the fragment. Not to be confused
            /// with the associated edit ID.
            id: FragmentId,
            insertion: Insertion,
            text: Text,

            /// The edit IDs of deletions that affect
            /// the text in this fragment. Instead of mutating
            /// this directly, use [`Self::mark_deletion`]
            /// to update the set of deletions.
            ///
            /// We use a [`BTreeSet`] to efficiently compute intersections
            /// with other sets (e.g. undo map).
            deletions: BTreeSet<Lamport>,

            /// Needed to ensure that text styles from a deleted fragment
            /// are only inherited one time. For example, if we delete all the way
            /// to the end of a style and then start typing, we want to first look to the
            /// most recently deleted style and inherit it once. This works to "save"
            /// what we were typing up to the deletion.
            text_style_inherited_after_deletion: bool,

            /// Whether or not the fragment is visible.
            ///
            /// This is cached on the fragment and must
            /// be recomputed when either
            /// 1. marking a deletion, or
            /// 2. undo'ing / redo'ing the edit or any of its deletions
            is_visible: bool,

            /// The maximum undo timestamp that affects
            /// this fragment (either the edit itself
            /// or any of its deletions), for each peer.
            max_undo_ts: Global,
        }

        #[derive(Eq, PartialEq, Clone, Debug)]
        pub struct FragmentSummary {
            text_summary: TextSummary,
            max_fragment_id: FragmentId,
            max_version: Global,
        }

        #[derive(Eq, PartialEq, Clone, Debug)]
        struct InsertionSplit {
            extent: CharOffset,
            fragment_id: FragmentId,
        }

        #[derive(Eq, PartialEq, Clone, Debug, Default)]
        struct InsertionSplitSummary {
            extent: CharOffset,
        }

        /// A CRDT payload for an edit operation.
        #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
        pub struct EditOperation {
            /// The lamport timestamp of this edit.
            lamport_timestamp: Lamport,

            /// The edits that had been observed when this edit was made.
            /// This intentionally excludes this edit (identified by [`Self::lamport_timestamp`]) itself.
            versions: Global,

            /// The edit ID that the start of this edit is relative to.
            /// The (start_id, start_character_offset) identify the start of
            /// the range that is being edited.
            start_id: Lamport,

            /// The offset from the edit identified by [`Self::start_id`].
            start_character_offset: CharOffset,

            /// The edit ID that the end of this edit is relative to.
            end_id: Lamport,

            /// The offset from the edit identified by [`Self::end_id`].
            /// The (end_id, end_character_offset) identify the end of
            /// the range that is being edited.
            end_character_offset: CharOffset,

            /// The replacement text.
            /// Note that this is a plain-old string;
            /// we do not support styling via CRDT operations.
            new_text: String,
        }

        /// A CRDT payload for an undo operation.
        #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
        pub struct UndoOperation {
            /// The lamport timestamp of the undo operation.
            /// Used to uniquely identify undo operations.
            lamport_timestamp: Lamport,

            /// The version vector at the time of the undo.
            versions: Global,

            /// The lamport timestamps of the operations being undone. A replica can only
            /// undo its own operation so the replica ID of each of these operations is lamport.replica_id.
            operations: Vec<LamportValue>,
        }

        struct OperationEditArgs {
            pub start_id: Option<Lamport>,
            pub start_character_offset: Option<CharOffset>,
            pub end_id: Option<Lamport>,
            pub end_character_offset: Option<CharOffset>,
            pub version_in_range: Global,
            new_text: String,
            pub lamport_timestamp: Lamport,
        }

        impl OperationEditArgs {
            pub fn new(
                new_text: Option<Text>,
                lamport_timestamp: Lamport,
                version_in_range: Global,
            ) -> Self {
                let new_text = if let Some(new_text) = new_text {
                    new_text.as_str().to_string()
                } else {
                    String::from("")
                };

                Self {
                    start_id: None,
                    start_character_offset: None,
                    end_id: None,
                    end_character_offset: None,
                    version_in_range,
                    new_text,
                    lamport_timestamp,
                }
            }

            pub fn update_end(&mut self, end_id: Lamport, end_offset: CharOffset) {
                self.end_id = Some(end_id);
                self.end_character_offset = Some(end_offset);
            }

            pub fn update_start(&mut self, start_id: Lamport, start_offset: CharOffset) {
                self.start_id = Some(start_id);
                self.start_character_offset = Some(start_offset);
            }
        }

        impl TryFrom<OperationEditArgs> for EditOperation {
            type Error = anyhow::Error;

            fn try_from(edit_args: OperationEditArgs) -> Result<Self> {
                let Some(start_id) = edit_args.start_id else {
                    anyhow::bail!("Tried to build EditOperation without start_id")
                };

                let Some(end_id) = edit_args.end_id else {
                    anyhow::bail!("Tried to build EditOperation without end_id")
                };

                let Some(start_character_offset) = edit_args.start_character_offset else {
                    anyhow::bail!("Tried to build EditOperation without start_character_offset")
                };

                let Some(end_character_offset) = edit_args.end_character_offset else {
                    anyhow::bail!("Tried to build EditOperation without end_character_offset")
                };

                Ok(EditOperation {
                    start_id,
                    start_character_offset,
                    end_id,
                    end_character_offset,
                    versions: edit_args.version_in_range,
                    new_text: edit_args.new_text,
                    lamport_timestamp: edit_args.lamport_timestamp,
                })
            }
        }

        struct SplicedFragment {
            chars_to_fragment_start: CharOffset,
            fragment: Fragment,
            insertion_split: InsertionSplit,
        }

        impl Buffer {
            pub(super) fn new_with_replica_id<T: Into<Text>>(
                replica_id: ReplicaId,
                base_text: T,
            ) -> Self {
                let mut insertion_splits = HashMap::new();
                let mut fragments = SumTree::new();

                // We use a dedicated, sentinel replica ID for the base edit so that all
                // peers in the system can apply it consistently.
                let base_replica_id = ReplicaId::base_replica_id();
                let base_insertion = Insertion {
                    id: Lamport::new(base_replica_id.clone()),
                    parent_id: Lamport::new(base_replica_id),
                    offset_in_parent: 0.into(),
                    text: base_text.into(),
                };

                insertion_splits.insert(
                    base_insertion.id.clone(),
                    SumTree::from_item(InsertionSplit {
                        fragment_id: FragmentId::min_value().clone(),
                        extent: 0.into(),
                    }),
                );
                fragments.push(Fragment {
                    id: FragmentId::min_value().clone(),
                    insertion: base_insertion.clone(),
                    text: base_insertion
                        .text
                        .slice(CharOffset::from(0)..CharOffset::from(0)),
                    deletions: BTreeSet::new(),
                    text_style_inherited_after_deletion: false,
                    is_visible: true,
                    max_undo_ts: Global::new(),
                });

                if !base_insertion.text.is_empty() {
                    let base_fragment_id =
                        FragmentId::between(FragmentId::min_value(), FragmentId::max_value());

                    insertion_splits
                        .get_mut(&base_insertion.id)
                        .unwrap()
                        .push(InsertionSplit {
                            fragment_id: base_fragment_id.clone(),
                            extent: base_insertion.text.len(),
                        });
                    fragments.push(Fragment {
                        id: base_fragment_id,
                        text: base_insertion.text.clone(),
                        insertion: base_insertion,
                        deletions: BTreeSet::new(),
                        text_style_inherited_after_deletion: false,
                        is_visible: true,
                        max_undo_ts: Global::new(),
                    });
                }

                // TODO: should the first cursor actually be at the end
                // of the buffer rather than at the start (in case there's
                // base text)?
                let root_selection = LocalSelection {
                    selection: Selection {
                        start: Anchor::Start,
                        end: Anchor::Start,
                        reversed: false,
                    },
                    clamp_direction: Default::default(),
                    goal_start_column: None,
                    goal_end_column: None,
                };
                let root_selections: LocalSelections = vec1![root_selection.clone()].into();

                Self {
                    fragments,
                    insertion_splits,
                    versions: Global::new(),
                    lamport_clock: Lamport::new(replica_id),
                    batch_state: BatchState::Inactive,
                    local_selections: root_selections.clone(),
                    local_undo_stack: LocalUndoStack::new(root_selections),
                    undo_history: UndoHistory::new(),
                }
            }

            pub(super) fn new<T: Into<Text>>(base_text: T) -> Self {
                Self::new_with_replica_id(ReplicaId::random(), base_text)
            }

            pub fn replica_id(&self) -> ReplicaId {
                self.lamport_clock.replica_id()
            }

            pub fn local_selections(&self) -> &LocalSelections {
                &self.local_selections
            }

            /// Initializes a batch for edits and selection changes;
            /// all further edits and selection changes will be batched.
            /// The batch must be completed via [`Self::end_batch`].
            ///
            /// TODO (suraj): investigate if we can use RAII to enforce the batch is ended
            /// when a guard (that would be returned by this fn) is dropped.
            /// The tricky part is how we would execute [`Self::on_batch_finished`] in
            /// a destructor.
            pub(super) fn start_edits_and_selection_changes_batch(
                &mut self,
                edit_origin: EditOrigin,
                action: PlainTextEditorViewAction,
                skip_undo: bool,
            ) {
                debug_assert!(!self.batch_state.is_batching());
                self.batch_state = BatchState::EditsAndSelectionChanges {
                    edit_origin,
                    action,
                    since: self.versions.clone(),
                    selections_before_batch: self.local_selections.clone(),
                    edits: vec![],
                    num_edits_already_recorded: 0,
                    skip_undo,
                };
            }

            pub(super) fn refresh_version_on_edits_and_selection_changes_batch(&mut self) {
                debug_assert!(self.batch_state.is_batching());
                if let BatchState::EditsAndSelectionChanges { since, .. } = &mut self.batch_state {
                    *since = self.versions.clone();
                }
            }

            /// Initializes a batch for selection-only changes;
            /// all further selection changes will be batched.
            /// The batch must be completed via [`Self::end_batch`].
            pub(super) fn start_selection_changes_only_batch(&mut self) {
                debug_assert!(!self.batch_state.is_batching());
                self.batch_state = BatchState::SelectionChangesOnly {
                    selections_before_batch: self.local_selections.clone(),
                };
            }

            fn start_undo_redo_batch(&mut self) {
                debug_assert!(!self.batch_state.is_batching());
                self.batch_state = BatchState::UndoRedo {
                    since: self.versions.clone(),
                    undo_op: None,
                };
            }

            /// Completes the on-going batch.
            pub(super) fn end_batch(&mut self, ctx: &mut ModelContext<Self>) {
                debug_assert!(self.batch_state.is_batching());
                let batch_state = std::mem::replace(&mut self.batch_state, BatchState::Inactive);

                match batch_state {
                    BatchState::Inactive => {
                        log::warn!("Tried to end batch when a batch wasn't ongoing");
                    }
                    BatchState::SelectionChangesOnly {
                        selections_before_batch,
                    } => {
                        if selections_before_batch != self.local_selections {
                            self.local_undo_stack
                                .record_selection_change(self.local_selections.clone());
                            ctx.emit(Event::SelectionsChanged);
                        }
                    }
                    BatchState::EditsAndSelectionChanges {
                        edits,
                        edit_origin,
                        action,
                        since,
                        num_edits_already_recorded,
                        skip_undo,
                        selections_before_batch,
                        ..
                    } => {
                        if !edits.is_empty() {
                            if !skip_undo {
                                let edit_lamport_values = edits
                                    .iter()
                                    .skip(num_edits_already_recorded)
                                    .map(|o: &EditOperation| o.lamport_timestamp.value);

                                self.local_undo_stack.record_edit(
                                    action,
                                    edit_lamport_values,
                                    self.local_selections.clone(),
                                );
                            }

                            let edit_ranges = self.edits_since(since).collect::<Vec<_>>();
                            ctx.emit(Event::Edited {
                                edits: edit_ranges,
                                edit_origin,
                            });
                        } else if selections_before_batch != self.local_selections {
                            // We only emit this event if we're not already emitting
                            // an [`Event::Edited`] event.
                            ctx.emit(Event::SelectionsChanged);
                        }
                    }
                    BatchState::UndoRedo { since, undo_op, .. } => {
                        if undo_op.is_some() {
                            let edit_ranges = self.edits_since(since).collect::<Vec<_>>();
                            ctx.emit(Event::Edited {
                                edits: edit_ranges,
                                edit_origin: EditOrigin::UserInitiated,
                            });
                        }
                    }
                }
            }

            pub fn snapshot(&self) -> EditorSnapshot {
                let selections = self
                    .local_selections
                    .selections
                    .mapped_ref(|selection| selection.to_offset(self));
                let buffer_text_runs = self.text_style_runs().collect();
                EditorSnapshot::new(selections, buffer_text_runs)
            }

            pub fn text_summary(&self) -> TextSummary {
                self.fragments.extent::<TextSummary>()
            }

            pub fn text_summary_for_range(&self, range: Range<CharOffset>) -> TextSummary {
                let mut summary = TextSummary::default();

                let mut cursor = self.fragments.cursor::<CharOffset, CharOffset>();
                cursor.seek(&range.start, SeekBias::Right);

                if let Some(fragment) = cursor.item() {
                    let summary_start = cmp::max(*cursor.start(), range.start) - *cursor.start();
                    let summary_end = cmp::min(range.end - *cursor.start(), fragment.len());
                    summary += &fragment.text.slice(summary_start..summary_end).summary();
                    cursor.next();
                }

                if range.end > *cursor.start() {
                    summary += &cursor.summary::<TextSummary>(&range.end, SeekBias::Right);

                    if let Some(fragment) = cursor.item() {
                        let summary_start =
                            cmp::max(*cursor.start(), range.start) - *cursor.start();
                        let summary_end = cmp::min(range.end - *cursor.start(), fragment.len());
                        summary += &fragment.text.slice(summary_start..summary_end).summary();
                    }
                }

                summary
            }

            pub fn len(&self) -> CharOffset {
                self.fragments.extent::<CharOffset>()
            }

            pub fn is_empty(&self) -> bool {
                self.len() == 0.into()
            }

            pub fn line_len(&self, row: u32) -> Result<u32> {
                let row_start_offset =
                    <Point as ToCharOffset>::to_char_offset(&Point::new(row, 0), self)?;
                let row_end_offset = if row >= self.max_point().row {
                    self.len()
                } else {
                    <Point as ToCharOffset>::to_char_offset(&Point::new(row + 1, 0), self)? - 1
                };

                Ok((row_end_offset.as_usize() - row_start_offset.as_usize()) as u32)
            }

            pub fn max_point(&self) -> Point {
                self.fragments.extent()
            }

            pub fn line(&self, row: u32) -> Result<String> {
                Ok(self
                    .chars_at(Point::new(row, 0))?
                    .take_while(|c| *c != '\n')
                    .collect())
            }

            pub fn text(&self) -> String {
                self.chars().collect()
            }

            pub fn text_for_range<T: ToCharOffset>(&self, range: Range<T>) -> Result<String> {
                Ok(self.chars_for_range(range)?.collect())
            }

            /// Iterator of text style runs of the buffer. Each style run is a string of text that
            /// shares the same text style.
            pub fn text_style_runs(&self) -> impl '_ + Iterator<Item = TextRun> {
                let mut fragments_cursor = self.fragments.cursor::<ByteOffset, ByteOffset>();
                let start = *fragments_cursor.start();
                fragments_cursor.seek(&start, SeekBias::Left);

                TextStyleRuns {
                    fragments_cursor,
                    byte_index_start: 0.into(),
                    byte_index_end: 0.into(),
                    undo_history: &self.undo_history,
                }
            }

            pub fn chars_for_range<T: ToCharOffset>(
                &self,
                range: Range<T>,
            ) -> Result<impl '_ + Iterator<Item = char>> {
                let start = range.start.to_char_offset(self)?;
                let end = range.end.to_char_offset(self)?;
                Ok(self
                    .chars_at(start)?
                    .take(end.as_usize() - start.as_usize()))
            }

            pub fn chars(&self) -> Chars<'_> {
                self.chars_at(CharOffset::from(0)).unwrap()
            }

            pub fn stylized_chars(&self) -> CharsWithStyle<'_> {
                self.stylized_chars_at(CharOffset::from(0))
                    .expect("offset 0 should exist")
            }

            pub fn chars_at<T: ToCharOffset>(&self, position: T) -> Result<Chars<'_>> {
                Ok(Chars(self.stylized_chars_at(position)?))
            }

            pub fn stylized_chars_at<T: ToCharOffset>(
                &self,
                position: T,
            ) -> Result<CharsWithStyle<'_>> {
                let offset = position.to_char_offset(self)?;

                let mut fragments_cursor = self.fragments.cursor::<CharOffset, CharOffset>();
                fragments_cursor.seek(&offset, SeekBias::Right);

                let fragment_offset = offset - *fragments_cursor.start();
                let (fragment_chars, fragment_text_style) = fragments_cursor.item().map_or(
                    ("".chars(), TextStyle::default()),
                    |fragment| {
                        (
                            fragment.text[fragment_offset..].chars(),
                            fragment.text.text_style().unwrap_or_default(),
                        )
                    },
                );

                Ok(CharsWithStyle {
                    fragments_cursor,
                    fragment_chars,
                    fragment_offset,
                    fragment_text_style,
                    reversed: false,
                    undo_history: &self.undo_history,
                })
            }

            /// Get an iterator of subword starting points forward from the given offset
            pub fn subword_starts_from_offset<T: BufferIndex>(
                &self,
                position: T,
            ) -> Result<SubwordBoundaries<'_, Self>> {
                let offset = position.to_char_offset(self)?;
                Ok(SubwordBoundaries::forward_subword_starts(
                    position.to_char_offset(self)?,
                    self.chars_at(offset)?,
                    self,
                ))
            }

            /// Get an iterator of subword ending points forward from the given offset,
            /// excluding the current location if it is a word boundary.
            pub fn subword_ends_from_offset_exclusive<T: BufferIndex>(
                &self,
                position: T,
            ) -> Result<SubwordBoundaries<'_, Self>> {
                let offset = position.to_char_offset(self)?;
                Ok(SubwordBoundaries::forward_subword_ends_exclusive(
                    position.to_char_offset(self)?,
                    self.chars_at(offset)?,
                    self,
                ))
            }

            /// Get an iterator of word starting points backwards from the given offset,
            /// excluding the current location if it is a word boundary.
            pub fn subword_backward_starts_from_offset_exclusive<T: BufferIndex>(
                &self,
                position: T,
            ) -> Result<SubwordBoundaries<'_, Self>> {
                let offset = position.to_char_offset(self)?;
                Ok(SubwordBoundaries::backward_subword_starts_exclusive(
                    position.to_char_offset(self)?,
                    self.chars_rev_at(offset)?,
                    self,
                ))
            }

            pub fn edits_since(&self, since: Global) -> impl '_ + Iterator<Item = Edit> {
                let since_2 = since.clone();
                let cursor = self
                    .fragments
                    .filter(move |summary| summary.max_version.changed_since(&since_2));

                Edits {
                    cursor,
                    since,
                    delta: 0,
                    undo_history: &self.undo_history,
                }
            }

            pub(super) fn indent(
                &mut self,
                row_range: Range<u32>,
                ctx: &mut ModelContext<Self>,
            ) -> Result<()> {
                let max_row = self.max_point().row;
                let offset_ranges = row_range
                    .map(|row| {
                        if row > max_row {
                            return Err(anyhow!("invalid row: {}", row));
                        }
                        let line_start = ToCharOffset::to_char_offset(&Point::new(row, 0), self)?;
                        Ok(line_start..line_start)
                    })
                    .collect::<Result<Vec<Range<CharOffset>>>>()?;
                self.edit(offset_ranges, "    ", ctx)
            }

            pub(super) fn unindent(
                &mut self,
                row_ranges: Vec<Range<u32>>,
                ctx: &mut ModelContext<Self>,
            ) -> Result<()> {
                let max_row = self.max_point().row;
                let offset_ranges = row_ranges
                    .into_iter()
                    .flat_map(|row_range| {
                        row_range.map(|row| {
                            if row > max_row {
                                return Err(anyhow!("invalid row: {}", row));
                            }
                            let tab_start =
                                ToCharOffset::to_char_offset(&Point::new(row, 0), self)?;
                            let tab_end = tab_start
                                + self
                                    .line(row)
                                    .unwrap()
                                    .chars()
                                    .take(4)
                                    .take_while(|c| c.is_whitespace())
                                    .count();
                            Ok(tab_start..tab_end)
                        })
                    })
                    .collect::<Result<Vec<Range<CharOffset>>>>()?;
                self.edit(offset_ranges, "", ctx)
            }

            /// Ticks the lamport timestamp and observes the time
            /// in the version vector.
            ///
            /// Returns the version vector _before_ the tick.
            fn tick(&mut self) -> Global {
                let version_before_tick = self.versions.clone();
                self.lamport_clock.tick();
                self.versions.observe(&self.lamport_clock);
                version_before_tick
            }

            pub(super) fn last_action(&self) -> Option<PlainTextEditorViewAction> {
                self.local_undo_stack.last_action()
            }

            pub(super) fn undo(&mut self, ctx: &mut ModelContext<Self>) {
                self.start_undo_redo_batch();

                if let Some((ops, selections)) = self.local_undo_stack.undo()
                    && let Err(e) = self.local_undo_or_redo(ops, selections)
                {
                    log::warn!("Failed to perform local undo: {e}");
                }

                self.end_batch(ctx);
            }

            pub(super) fn redo(&mut self, ctx: &mut ModelContext<Self>) {
                self.start_undo_redo_batch();

                if let Some((ops, selections)) = self.local_undo_stack.redo()
                    && let Err(e) = self.local_undo_or_redo(ops, selections)
                {
                    log::warn!("Failed to perform local redo: {e}");
                }

                self.end_batch(ctx);
            }

            pub(super) fn record_edits(
                &mut self,
                action: PlainTextEditorViewAction,
                _ctx: &mut ModelContext<Self>,
            ) {
                if let BatchState::EditsAndSelectionChanges {
                    edits,
                    num_edits_already_recorded,
                    ..
                } = &mut self.batch_state
                {
                    let lamport_values = edits
                        .iter()
                        .skip(*num_edits_already_recorded)
                        .map(|o| o.lamport_timestamp.value);
                    self.local_undo_stack.record_edit(
                        action,
                        lamport_values,
                        self.local_selections.clone(),
                    );
                    *num_edits_already_recorded = edits.len();
                } else {
                    debug_assert!(
                        false,
                        "Tried to record edits in an insufficient batch state"
                    );
                }
            }

            pub(super) fn reset_undo_stack(&mut self) {
                self.local_undo_stack.reset(self.local_selections.clone());
            }

            /// Undoes or redoes the `edits` (based on what each edit's current
            /// state is; see [`Self::undo_or_redo`]) and restores the selection
            /// state to `selections`.
            fn local_undo_or_redo(
                &mut self,
                edits: Vec<LamportValue>,
                selections: LocalSelections,
            ) -> Result<()> {
                self.batch_state.attempt_undo()?;

                let since = self.tick();
                self.undo_or_redo(self.lamport_clock.clone(), edits.clone())?;
                self.change_selections(selections)?;

                self.batch_state.set_undo_op(UndoOperation {
                    lamport_timestamp: self.lamport_clock.clone(),
                    operations: edits,
                    versions: since.clone(),
                });
                Ok(())
            }

            /// Undoes or redoes the `edits` with the given `undo_ts``.
            ///
            /// For each edit:
            /// - if it is undone, then we would redo it.
            /// - if it is redone (or was never undone), we would undo it.
            fn undo_or_redo(&mut self, undo_ts: Lamport, edits: Vec<LamportValue>) -> Result<()> {
                // Register all of the edits that should be undone.
                let mut undone_edits = BTreeSet::new();
                for edit in edits {
                    let l = Lamport {
                        replica_id: undo_ts.replica_id(),
                        value: edit,
                    };
                    undone_edits.insert(l.clone());
                    self.undo_history.undo(l, undo_ts.value);
                }

                // TODO: rather than iterating over each fragment and checking
                // if that fragment needs to be updated, we should just search
                // for all the relevant fragments directly.
                let old_fragments = self.fragments.clone();
                let mut new_fragments = SumTree::new();
                let mut cursor = old_fragments.cursor::<FragmentIdRef, ()>();
                cursor.seek(&FragmentIdRef::new(FragmentId::min_value()), SeekBias::Left);

                while let Some(mut fragment) = cursor.item().cloned() {
                    // If the edit associated to this fragment or
                    // any of its deletions were undone, we should
                    // 1. recompute whether this fragment is visible
                    //    because whether a fragment is visible depends
                    //    on if the fragment or its deletions were undone
                    //    (see [`Fragment::is_visible`]) and we just marked
                    //    edits as undone above
                    // 2. reflect the undo in the fragment's max undo ts
                    if undone_edits.contains(&fragment.insertion.id)
                        || !fragment.deletions.is_disjoint(&undone_edits)
                    {
                        fragment.recompute_visibility(&self.undo_history);
                        fragment.max_undo_ts.observe(&undo_ts);
                    }

                    new_fragments.push(fragment);
                    cursor.next();
                }
                self.fragments = new_fragments;

                // After apply an undo / redo operation, we need to merge selection state
                // in case there are overlapping ranges.
                self.merge_all_selections()?;

                Ok(())
            }

            pub fn versions(&self) -> Global {
                self.versions.clone()
            }

            /// Changes the set of local selections to the ones provided,
            /// and post-processes them to ensure that any contiguous selections are merged.
            /// Selections can only be changed while batching (see [`Self::start_batch`]).
            ///
            /// TODO: ideally, this method can take in a callback that operates over
            /// a mutable [`LocalSelections`] to avoid requiring the caller to allocate a new
            /// set of [`LocalSelections`]. But this leads to borrow errors because a lot of
            /// selection changes require querying the [`Buffer`].
            pub(super) fn change_selections(
                &mut self,
                new_selections: LocalSelections,
            ) -> Result<()> {
                // Make sure this change is part of a batch.
                self.batch_state.attempt_selection_change()?;

                self.local_selections = new_selections;
                self.merge_local_selections()?;
                Ok(())
            }

            /// Replaces the old ranges with the new_text. Note that old_ranges MUST be sorted and have
            /// unique values, otherwise the call will panick.
            pub(super) fn edit<I, S, T>(
                &mut self,
                old_ranges: I,
                new_text: T,
                _ctx: &mut ModelContext<Self>,
            ) -> Result<()>
            where
                I: IntoIterator<Item = Range<S>>,
                S: ToCharOffset,
                T: Into<Text>,
            {
                // Make sure this edit is part of a batch.
                self.batch_state.attempt_edit()?;

                let new_text = new_text.into();
                let new_text = if !new_text.is_empty() {
                    Some(new_text)
                } else {
                    None
                };

                let old_ranges = old_ranges
                    .into_iter()
                    .map(|range| {
                        Ok(range.start.to_char_offset(self)?..range.end.to_char_offset(self)?)
                    })
                    .collect::<Result<Vec<Range<CharOffset>>>>()?;

                let ops = self.update_fragments_for_edit(old_ranges, new_text.clone())?;

                if let Some(op) = ops.last() {
                    self.versions.observe(&op.lamport_timestamp);
                }

                self.batch_state.extend_edits(ops);

                // After editing the buffer, we need to merge selection state
                // in case there are overlapping ranges.
                self.merge_all_selections()?;

                Ok(())
            }

            /// Updates the text styles for text within a given buffer
            /// in-place (by updating the underlying Fragments). Note that the caller
            /// can pass in multiple ranges at once if they'd like to apply the same
            /// style to each of the ranges.
            ///
            /// # Arguments
            ///
            /// * `old_ranges` - An object that can be iterated over, containing Ranges
            ///   defined by CharOffsets.
            /// * `text_style_operation` - The TextStyleOperation to apply to the given text ranges.
            /// * `ctx` - ModelContext for the buffer being updated.
            ///
            /// # Example
            /// ```ignore
            /// use warpui::{color::ColorU, App, ModelHandle};
            /// use warp::Assets;
            /// use warp::editor::model::buffer::{Buffer, TextStyleOperation, EditOrigin};
            /// use string_offset::CharOffset;
            /// App::test((), |mut app| async move {
            ///     let buffer_model: &mut ModelHandle<Buffer> =
            ///     &mut app.add_model(|_| Buffer::new(0, "abc"));
            ///     buffer_model.update(&mut app, |buffer, ctx| {
            ///         buffer.update_styles(
            ///             vec![CharOffset::from(0)..CharOffset::from(2)],
            ///             TextStyleOperation::default().set_error_underline_color(ColorU::black()),
            ///             EditOrigin::UserInitiated,
            ///             ctx,
            ///         );
            ///     });
            /// });
            /// ```
            ///
            pub(super) fn update_styles<I, S>(
                &mut self,
                old_ranges: I,
                text_style_operation: TextStyleOperation,
                ctx: &mut ModelContext<Self>,
            ) -> Result<()>
            where
                I: IntoIterator<Item = Range<S>>,
                S: ToCharOffset,
            {
                // We don't need to be in a txn when updating styles because
                // 1. it doesn't actually change the contents of the buffer (including selection state)
                // 2. it is not a CRDT-compliant change
                let old_ranges = old_ranges
                    .into_iter()
                    .map(|range| {
                        Ok(range.start.to_char_offset(self)?..range.end.to_char_offset(self)?)
                    })
                    .collect::<Result<Vec<Range<CharOffset>>>>()?;

                self.update_fragment_styles(
                    old_ranges
                        .into_iter()
                        .filter(|old_range| old_range.end > old_range.start),
                    text_style_operation,
                )?;

                ctx.emit(Event::StylesUpdated);
                Ok(())
            }

            /// Splices `current_fragment` into a new spliced fragment at `char_offset`, updating `fragment`
            /// to start at `char_offset`. Note, both `char_offset` and `chars_to_fragment_start` are
            /// character offsets from the _start_ of the buffer. For example, if `current_fragment` goes
            /// from character offset 10 to 20 and `char_offset` is 15, `current_fragment` would now start
            /// at 15 and a new spliced fragment would be returned that goes from 10 -> 15. Note it is up to
            /// the caller to ensure that `char_offset` is contained within `fragment`.
            fn splice_fragment_at_char_offset(
                current_fragment: &mut Fragment,
                last_fragment: &Fragment,
                chars_to_fragment_start: CharOffset,
                char_offset: CharOffset,
            ) -> SplicedFragment {
                let mut prefix = current_fragment.clone();

                prefix.set_end_character_offset(
                    prefix.start_character_offset() + (char_offset - chars_to_fragment_start),
                );
                prefix.id = FragmentId::between(&last_fragment.id, &current_fragment.id);
                current_fragment.set_start_offset(prefix.end_character_offset());

                let insertion_split = InsertionSplit {
                    extent: prefix.end_character_offset() - prefix.start_character_offset(),
                    fragment_id: prefix.id.clone(),
                };

                SplicedFragment {
                    chars_to_fragment_start: char_offset,
                    fragment: prefix,
                    insertion_split,
                }
            }

            /// Inserts `new_text` as a new fragment into the buffer, splicing fragments as necessary and
            /// returns a Vector of `Operations` of the edits that were made.
            ///
            /// The buffer is composed is a series of `Fragments` (essentially a range of text, and a
            /// timestamp to indicate when/if the fragment was deleted). At a high level, this function
            /// builds up a new set of fragments that make up the model after the edit. All fragments before
            /// the first item in the range or after any of the ranges are considered to be untouched. If a
            /// range starts or ends within a fragment, that fragment is sliced into 2 fragments (one for the
            /// piece up to the range that is now considered deleted), and one for the piece of the fragment
            /// that is not covered by the range. Any fragments that are fully contained within a range are
            /// considered to be fully deleted.
            ///
            /// Note it is up to the caller to ensure that `old_ranges` is sorted by the start of the range
            /// ranges are mutually exclusive and that the start point is <= the end point for each range.
            fn update_fragments_for_edit(
                &mut self,
                old_ranges: Vec<Range<CharOffset>>,
                new_text: Option<Text>,
            ) -> Result<Vec<EditOperation>> {
                debug_assert!(
                    old_ranges.iter().all(|r| r.start <= r.end)
                        && old_ranges
                            .windows(2)
                            .all(|w| w[0].start < w[1].start && w[0].end < w[1].start)
                );

                let mut old_ranges = old_ranges.into_iter().filter(|range| {
                    // When new_text is some, that means we're inserting text or replacing text,
                    // so any given range could be empty or non-empty.
                    // When removing text, the range should be non-empty (removing nothing is not useful).
                    if new_text.is_some() {
                        range.start <= range.end
                    } else {
                        range.start < range.end
                    }
                });

                let cur_range = match old_ranges.next() {
                    None => return Ok(Vec::new()),
                    Some(range) => range,
                };
                let mut ops = Vec::with_capacity(old_ranges.size_hint().0);

                let old_fragments = self.fragments.clone();
                let mut cursor = old_fragments.cursor::<CharOffset, CharOffset>();
                let mut new_fragments = SumTree::new();

                // Push all parts of the existing fragments SumTree that occur _before_ the start of the
                // current range since these fragments are unaffected by the current edit.
                new_fragments.push_tree(cursor.slice(&cur_range.start, SeekBias::Right));

                // Since ranges is non-empty, we know we're going to produce at least one edit.
                // Note that the version vector that we use for the edit payload _does_ not include
                // this edit, while the lamport clock _does_ reflect this edit. This is also true
                // for other edit ops we might end up producing as part of this function.
                let mut version = self.tick();

                let mut operation_edit_args = OperationEditArgs::new(
                    new_text.clone(),
                    self.lamport_clock.clone(),
                    version.clone(),
                );

                let mut cur_range = Some(cur_range);

                // We loop through all fragments that are either fully within the current range
                // or intersect with the current range. Note that we're using a 2 pointers approach
                // with the current range and current fragment (we move our current fragment until
                // we're outside of the current range and then we move onto the next range).
                while let Some(mut current_fragment) = cur_range
                    .is_some()
                    .then(|| cursor.item().cloned())
                    .flatten()
                {
                    // Number of characters the start of the fragment is offset from the beginning of the
                    // buffer.
                    let mut chars_to_fragment_start = *cursor.start();

                    // Number of characters the end of the fragment is offset from the start of the
                    // buffer.
                    let mut chars_to_fragment_end =
                        chars_to_fragment_start + current_fragment.visible_len(&self.undo_history);

                    // For both cases (fully contained fragment or intersecting), we delete the original fragment insertion.
                    // In the case of intersecting fragments, we will split this fragment into 2 parts.
                    // In the case of fully contained fragments, we will entirely be replacing the fragment with our new range.
                    let old_split_tree = self
                        .insertion_splits
                        .remove(&current_fragment.insertion.id)
                        .ok_or_else(|| {
                            anyhow!("Current fragment does not exist in insertion_splits")
                        })?;

                    let mut splits_cursor = old_split_tree.cursor::<CharOffset, ()>();
                    // Initialize a new SumTree for the current fragment which we are splitting.
                    let mut new_split_tree = splits_cursor
                        .slice(&current_fragment.start_character_offset(), SeekBias::Right);

                    // Find all splices that start or end within the current fragment. Then, split the
                    // fragment and reassemble it in both trees accounting for the deleted and the newly
                    // inserted text.
                    while let Some(range) = cur_range
                        .as_ref()
                        .filter(|range| range.start < chars_to_fragment_end)
                    {
                        Self::maybe_splice_fragment(
                            range,
                            &mut current_fragment,
                            &mut new_split_tree,
                            &mut new_fragments,
                            &mut chars_to_fragment_start,
                        )?;

                        // If the end of the current range is equal to the start of the current fragment, our
                        // edit operation's end should be the end of the last fragment.
                        // Otherwise, if the end of the current range is equal to the end of the current fragment,
                        // our edit operation's end should be the end of the current fragment.
                        if range.end == chars_to_fragment_start {
                            let last_fragment = new_fragments.last().ok_or_else(|| {
                                anyhow!("New fragment tree should have at least one item")
                            })?;
                            operation_edit_args.update_end(
                                last_fragment.insertion.id.clone(),
                                last_fragment.end_character_offset(),
                            );
                        } else if range.end == chars_to_fragment_end {
                            operation_edit_args.update_end(
                                current_fragment.insertion.id.clone(),
                                current_fragment.end_character_offset(),
                            );
                        }

                        // The range is at a fragment boundary. Insert the text as a new fragment before the
                        // current fragment.
                        if range.start == chars_to_fragment_start {
                            let last_fragment = new_fragments.last().ok_or_else(|| {
                                anyhow!("New fragment tree should have at least one item")
                            })?;
                            // The start point for our edit operation is the end of the last fragment.
                            operation_edit_args.update_start(
                                last_fragment.insertion.id.clone(),
                                last_fragment.end_character_offset(),
                            );

                            if let Some(mut new_text) = new_text.clone() {
                                // If the new fragment is going to be inserted at a fragment boundary, then look
                                // at the left most visible or recently deleted fragment. Otherwise, we want to inherit
                                // the style from the current fragment since we are splitting it.
                                new_text.fallback_text_style_with(|| {
                                    // Note this means chars_to_fragment_start == range.start == range.end
                                    // i.e. this range is a single insertion point. Hence, we try to inherit the style (left/recently deleted).
                                    // Example: "foobar" where "foo" is highlighted. The user starts typing right after "foo" - we would expect to retain the
                                    // highlight. In that example, the range is 3..3
                                    if chars_to_fragment_start == range.end {
                                        self.style_from_left_visible_or_recently_deleted_fragment(
                                            &mut new_fragments,
                                        )
                                    } else {
                                        // Otherwise, in this case, we're splitting the current fragment so adopt it's style.
                                        // Example: "foobar" where "bar" is highlighted. The user then selects "b" and pastes in "xyz".
                                        // At this point, we expect "fooxyzar" where "xyzar" is highlighted.
                                        current_fragment
                                            .text
                                            .text_style
                                            .map(TextStyle::filter_inheritable_styles)
                                    }
                                });
                                let new_fragment = self.build_fragment_to_insert(
                                    new_fragments.last().ok_or_else(|| {
                                        anyhow!("New fragment tree should have at least one item")
                                    })?,
                                    Some(&current_fragment),
                                    new_text,
                                    self.lamport_clock.clone(),
                                );
                                new_fragments.push(new_fragment);
                            }
                        }

                        // Check if the current fragment can be fully deleted or if it has to be spliced.
                        if let Some(spliced_fragment) = self.maybe_mark_fragment_as_deleted(
                            &mut current_fragment,
                            chars_to_fragment_start,
                            chars_to_fragment_end,
                            new_fragments.last().ok_or_else(|| {
                                anyhow!("New fragment tree should have at least one item")
                            })?,
                            range,
                            &mut operation_edit_args,
                        ) {
                            // If we had to delete a spliced fragment, we update our current position
                            // (note that this will actually be range.end).
                            chars_to_fragment_start = spliced_fragment.chars_to_fragment_start;
                            new_split_tree.push(spliced_fragment.insertion_split);
                            new_fragments.push(spliced_fragment.fragment)
                        }

                        // If the current range ends inside this fragment, we can advance to the next range
                        // and check if it also intersects the current fragment. Otherwise we break out of
                        // the loop and find the first fragment that the range does not contain fully.
                        if range.end <= chars_to_fragment_end {
                            ops.push(operation_edit_args.try_into()?);
                            // We move onto the next range.
                            cur_range = old_ranges.next();
                            // For more precise timestamps on edits (since this loop will take some
                            // time to execute).
                            if cur_range.is_some() {
                                version = self.tick();
                            }
                            // We initialize our new edit operation for the next range.
                            operation_edit_args = OperationEditArgs::new(
                                new_text.clone(),
                                self.lamport_clock.clone(),
                                version.clone(),
                            );
                        } else {
                            break;
                        }
                    }

                    // We're done with the current fragment - save relevant data to structs.
                    new_split_tree.push(InsertionSplit {
                        extent: current_fragment.end_character_offset()
                            - current_fragment.start_character_offset(),
                        fragment_id: current_fragment.id.clone(),
                    });
                    splits_cursor.next(); // Move onto the next insertion split.
                    new_split_tree.push_tree(
                        splits_cursor
                            .slice(&old_split_tree.extent::<CharOffset>(), SeekBias::Right),
                    );
                    self.insertion_splits
                        .insert(current_fragment.insertion.id.clone(), new_split_tree);
                    new_fragments.push(current_fragment);

                    // Continue scanning forward until we find a fragment that is not fully contained by the
                    // current splice. Mark each intermediate fragment as deleted.
                    cursor.next();
                    if let Some(range) = cur_range.clone() {
                        let range_ends_at_fragment_boundary = self.mark_fragments_as_deleted(
                            &mut cursor,
                            &mut new_fragments,
                            &mut operation_edit_args,
                            &mut chars_to_fragment_start,
                            &mut chars_to_fragment_end,
                            range,
                        );

                        // If the range ends at a fragment boundary, we've marked all necessary fragments as
                        // deleted and we can continue updating the fragments for the next range. If the
                        // range does NOT end at a fragment boundary, we continue the current while loop so
                        // the last fragment can be spliced appropriately.
                        if range_ends_at_fragment_boundary {
                            ops.push(operation_edit_args.try_into()?);

                            // Move onto the next range + start a new edit operation.
                            cur_range = old_ranges.next();
                            if cur_range.is_some() {
                                version = self.tick();
                            }

                            operation_edit_args = OperationEditArgs::new(
                                new_text.clone(),
                                self.lamport_clock.clone(),
                                version.clone(),
                            );
                        }

                        // If the range we are currently evaluating starts after the end of the fragment
                        // that the cursor is parked at, we should seek to the next splice's start range
                        // and push all the fragments in between into the new tree (they are untouched).
                        if let Some(cur_range) = cur_range.as_ref()
                            && cur_range.start > chars_to_fragment_end
                        {
                            new_fragments
                                .push_tree(cursor.slice(&cur_range.start, SeekBias::Right));
                        }
                    }
                }

                // Handle range that is at the end of the buffer if it exists. There should never be
                // multiple because ranges must be disjoint.
                // Example: we have a buffer of "foo" and cur_range is 3..3 - we need to add
                // a last fragment to the end.
                if cur_range.is_some() {
                    debug_assert_eq!(old_ranges.next(), None);
                    let last_fragment = new_fragments.last().ok_or_else(|| {
                        anyhow!("New fragment tree should have at least one item")
                    })?;
                    operation_edit_args = OperationEditArgs::new(
                        new_text.clone(),
                        self.lamport_clock.clone(),
                        version.clone(),
                    );
                    // Our edit operation's offsets are equal (single point edit).
                    operation_edit_args.update_start(
                        last_fragment.insertion.id.clone(),
                        last_fragment.end_character_offset(),
                    );
                    operation_edit_args.update_end(
                        last_fragment.insertion.id.clone(),
                        last_fragment.end_character_offset(),
                    );
                    ops.push(operation_edit_args.try_into()?);

                    if let Some(mut new_text) = new_text {
                        new_text.fallback_text_style_with(|| {
                            // In this case, there's only the one possibility since we're at the end of the
                            // buffer - hence we inherit styles from left/recently deleted (no current
                            // fragment that we're splitting).
                            self.style_from_left_visible_or_recently_deleted_fragment(
                                &mut new_fragments,
                            )
                        });
                        let last_fragment = new_fragments.last().ok_or_else(|| {
                            anyhow!("New fragment tree should have at least one item")
                        })?;
                        let new_fragment = self.build_fragment_to_insert(
                            last_fragment,
                            None,
                            new_text,
                            self.lamport_clock.clone(),
                        );
                        new_fragments.push(new_fragment);
                    }
                } else {
                    // Capture all remaining untouched fragments and add them into our new SumTree.
                    new_fragments.push_tree(
                        cursor.slice(&old_fragments.extent::<CharOffset>(), SeekBias::Right),
                    );
                }

                self.fragments = new_fragments;
                Ok(ops)
            }

            /// Updates the desired ranges by applying `text_style_operation`, splicing fragments as necessary.
            ///
            /// This function is similar to `update_fragments_for_edit` above, except it focuses on
            /// updating the styles for these ranges in-place and does not produce Edit Operations.
            ///
            /// At a high level, this function builds up a new set of fragments that make up the model after the edit.
            /// All fragments before the first item in the range or after any of the ranges are considered to be untouched.
            /// If a range starts or ends within a fragment, that fragment is sliced into 2 fragments (one for the
            /// piece up to the range that is to be styled), and one for the piece of the fragment
            /// that is not covered by the range. Any fragments that are fully contained within a range get their
            /// styles updated appropriately.
            ///
            /// Note it is up to the caller to ensure that `old_ranges` is sorted by the start of the range.
            fn update_fragment_styles<I>(
                &mut self,
                mut old_ranges: I,
                text_style_operation: TextStyleOperation,
            ) -> Result<()>
            where
                I: Iterator<Item = Range<CharOffset>>,
            {
                let cur_range = match old_ranges.next() {
                    None => return Ok(()),
                    Some(range) => range,
                };

                let old_fragments = self.fragments.clone();

                let mut cursor = old_fragments.cursor::<CharOffset, CharOffset>();
                let mut new_fragments = SumTree::new();

                // Push all parts of the existing fragments SumTree that occur _before_ the start of the
                // current range since these fragments are unaffected by the current edit.
                new_fragments.push_tree(cursor.slice(&cur_range.start, SeekBias::Right));

                let mut cur_range = Some(cur_range);

                // We loop through all fragments that are either fully within the current range
                // or intersect with the current range.
                while let Some(mut current_fragment) = cur_range
                    .is_some()
                    .then(|| cursor.item().cloned())
                    .flatten()
                {
                    // Number of characters the start of the current fragment is offset from the beginning of the
                    // buffer.
                    let mut chars_to_fragment_start = *cursor.start();

                    // Number of characters the end of the current fragment is offset from the start of the
                    // buffer.
                    let mut chars_to_fragment_end =
                        chars_to_fragment_start + current_fragment.visible_len(&self.undo_history);

                    let old_split_tree = self
                        .insertion_splits
                        .remove(&current_fragment.insertion.id)
                        .ok_or_else(|| {
                            anyhow!("Current fragment does not exist in insertion_splits")
                        })?;

                    let mut splits_cursor = old_split_tree.cursor::<CharOffset, ()>();
                    let mut new_split_tree = splits_cursor
                        .slice(&current_fragment.start_character_offset(), SeekBias::Right);

                    // Find all splices that start or end within the current fragment. Then, split the
                    // fragment and reassemble it in both trees accounting for the styled and unstyled text.
                    while let Some(range) = cur_range
                        .as_ref()
                        .filter(|range| range.start < chars_to_fragment_end)
                    {
                        Self::maybe_splice_fragment(
                            range,
                            &mut current_fragment,
                            &mut new_split_tree,
                            &mut new_fragments,
                            &mut chars_to_fragment_start,
                        )?;

                        // Covers the case of range covering the entire current fragment or partial (range ends inside fragment)
                        // At this point, we know that chars_to_fragment_start is the start of the range and start of the current fragment
                        // (potentially spliced from before) - we style the relevant portion of the fragment.
                        if let Ok(Some(spliced_fragment)) = self.maybe_style_fragment(
                            &mut current_fragment,
                            &mut new_fragments,
                            chars_to_fragment_start,
                            chars_to_fragment_end,
                            range,
                            text_style_operation,
                        ) {
                            new_split_tree.push(spliced_fragment.insertion_split);
                            new_fragments.push(spliced_fragment.fragment);
                            // If spliced, we move our start pointer over
                            chars_to_fragment_start = spliced_fragment.chars_to_fragment_start;
                        }

                        // If the current range ends inside this fragment, we can advance to the next range
                        // and check if it also intersects the current fragment. Otherwise we break out of
                        // the loop and find the first fragment that the range does not contain fully.
                        if range.end <= chars_to_fragment_end {
                            cur_range = old_ranges.next();
                        } else {
                            break;
                        }
                    }

                    new_split_tree.push(InsertionSplit {
                        extent: current_fragment.end_character_offset()
                            - current_fragment.start_character_offset(),
                        fragment_id: current_fragment.id.clone(),
                    });
                    splits_cursor.next();
                    new_split_tree.push_tree(
                        splits_cursor
                            .slice(&old_split_tree.extent::<CharOffset>(), SeekBias::Right),
                    );
                    self.insertion_splits
                        .insert(current_fragment.insertion.id.clone(), new_split_tree);
                    // We add in the current fragment (along with insertion split above).
                    new_fragments.push(current_fragment);

                    // Continue scanning forward until we find a fragment that is not fully contained by the
                    // current splice. We style each intermediate fragment (with `text_style_operation`).
                    cursor.next();
                    if let Some(range) = cur_range.clone() {
                        let range_ends_at_fragment_boundary = self.style_intermediate_fragments(
                            &mut cursor,
                            &mut new_fragments,
                            &mut chars_to_fragment_start,
                            &mut chars_to_fragment_end,
                            range,
                            text_style_operation,
                        );

                        // If the range ends at a fragment boundary, we've marked all necessary fragments as
                        // deleted and we can continue updating the fragments for the next range. If the
                        // range does NOT end at a fragment boundary, we continue the current while loop so
                        // the last fragment can be spliced appropriately.
                        if range_ends_at_fragment_boundary {
                            cur_range = old_ranges.next();
                        }

                        // If the range we are currently evaluating starts after the end of the fragment
                        // that the cursor is parked at, we should seek to the next splice's start range
                        // and push all the fragments in between into the new tree.
                        if let Some(cur_range) = cur_range.as_ref()
                            && cur_range.start > chars_to_fragment_end
                        {
                            new_fragments
                                .push_tree(cursor.slice(&cur_range.start, SeekBias::Right));
                        }
                    }
                }

                // Add remaining fragments (note range at end of buffer is no-op for update styles).
                new_fragments.push_tree(
                    cursor.slice(&old_fragments.extent::<CharOffset>(), SeekBias::Right),
                );

                self.fragments = new_fragments;
                Ok(())
            }

            fn maybe_splice_fragment(
                range: &Range<CharOffset>,
                fragment: &mut Fragment,
                new_split_tree: &mut SumTree<InsertionSplit>,
                new_fragments: &mut SumTree<Fragment>,
                chars_to_fragment_start: &mut CharOffset,
            ) -> Result<()> {
                // The range intersects a fragment -- this means we need to splice the fragment at
                // the correct offset. For example a fragment with the text "foo" starting at 0
                // would be spliced into "fo" if the start of the range was 2.
                if range.start > *chars_to_fragment_start {
                    // Note that the current_fragment is overwritten to be the latter part of the splice
                    // whereas the spliced_fragment contains the earlier part of the splice.
                    // The spliced fragment should not be styled.
                    let spliced_fragment = Self::splice_fragment_at_char_offset(
                        fragment,
                        new_fragments.last().ok_or_else(|| {
                            anyhow!("New fragment tree should have at least one item")
                        })?,
                        *chars_to_fragment_start,
                        range.start,
                    );
                    *chars_to_fragment_start = spliced_fragment.chars_to_fragment_start;
                    new_split_tree.push(spliced_fragment.insertion_split);
                    new_fragments.push(spliced_fragment.fragment);
                    return Ok(());
                }
                Ok(())
            }

            /// Marks `fragment` as deleted if the `range` exceeds past the Fragment. If the range
            /// ends within the fragment, the fragment is spliced and only the part that overlaps is marked
            /// as deleted.
            fn maybe_mark_fragment_as_deleted(
                &self,
                fragment: &mut Fragment,
                chars_to_fragment_start: CharOffset,
                chars_to_fragment_end: CharOffset,
                last_fragment: &Fragment,
                range: &Range<CharOffset>,
                operation_edit_args: &mut OperationEditArgs,
            ) -> Option<SplicedFragment> {
                if range.end < chars_to_fragment_end {
                    if range.end > chars_to_fragment_start {
                        // The range ends somewhere within the current fragment. Slice out the part
                        // of the fragment before the end of the range as deleted.

                        let mut spliced_fragment = Self::splice_fragment_at_char_offset(
                            fragment,
                            last_fragment,
                            chars_to_fragment_start,
                            range.end,
                        );

                        // Mark the spliced fragment as deleted.
                        spliced_fragment
                            .fragment
                            .mark_deletion(self.lamport_clock.clone(), &self.undo_history);
                        operation_edit_args.update_end(
                            fragment.insertion.id.clone(),
                            fragment.start_character_offset(),
                        );
                        operation_edit_args
                            .version_in_range
                            .observe(&fragment.insertion.id);

                        return Some(spliced_fragment);
                    }
                    // If range.end < chars_to_fragment_start, we shouldn't be trying to delete/splice anyhow (fragment is untouched).
                    // If range.end == chars_to_fragment_start, we apply edit operations with the last fragment's end character (see main body of caller function).
                } else {
                    // The range ends beyond the current fragment, this means we can mark the entire
                    // fragment as deleted and move on to find the fragment where the range ends.
                    operation_edit_args
                        .version_in_range
                        .observe(&fragment.insertion.id);
                    fragment.mark_deletion(self.lamport_clock.clone(), &self.undo_history);
                }

                None
            }

            /// Styles `fragment` entirely if the `range` exceeds past the Fragment. If the range
            /// ends within the fragment, the fragment is spliced and only the part that overlaps is
            /// styled.
            ///
            /// We have the following possibilities for return values (of type Result<Option<SplicedFragment>>):
            /// 1. Some(SplicedFragment) - the range splices the current fragment
            ///    i.e. the end of the range falls within the current fragment. We return
            ///    the new spliced fragment which is the earlier part (we set the appropriate
            ///    new text style).
            /// 2. The function returns an error - this only happens if the range splices
            ///    the current fragment and new_fragments has 0 items, which should never occur
            ///    (in a practical sense).
            /// 3. None - the range entirely covers the current fragment meaning we can
            ///    style the current fragment in-place (note that the caller already
            ///    has a reference to the current fragment hence we don't return it).
            fn maybe_style_fragment(
                &mut self,
                fragment: &mut Fragment,
                new_fragments: &mut SumTree<Fragment>,
                chars_to_fragment_start: CharOffset,
                chars_to_fragment_end: CharOffset,
                range: &Range<CharOffset>,
                text_style_operation: TextStyleOperation,
            ) -> Result<Option<SplicedFragment>> {
                if range.end < chars_to_fragment_end {
                    if range.end > chars_to_fragment_start {
                        // The range ends somewhere within the current fragment. Slice out the part
                        // of the fragment before the end of the range and style it appropriately.
                        let mut spliced_fragment = Self::splice_fragment_at_char_offset(
                            fragment,
                            new_fragments.last().ok_or_else(|| {
                                anyhow!("New fragment tree should have at least one item")
                            })?,
                            chars_to_fragment_start,
                            range.end,
                        );
                        spliced_fragment.fragment.set_text_style(
                            TextStyleOperation::apply_text_style_operation(
                                spliced_fragment
                                    .fragment
                                    .text
                                    .text_style
                                    .unwrap_or_default(),
                                text_style_operation,
                            ),
                        );
                        return Ok(Some(spliced_fragment));
                    }
                } else {
                    // Mutate the current fragment to update the styles for the text.
                    let cur_text = fragment.text.clone();
                    fragment.set_text_style(TextStyleOperation::apply_text_style_operation(
                        cur_text.text_style.unwrap_or_default(),
                        text_style_operation,
                    ));
                }
                Ok(None)
            }

            /// Mark all fragments as deleted that are completely covered by the current `range`. Returns
            /// whether the range ends exactly at a fragment boundary.
            fn mark_fragments_as_deleted(
                &mut self,
                cursor: &mut Cursor<Fragment, CharOffset, CharOffset>,
                new_fragments: &mut SumTree<Fragment>,
                operation_edit_args: &mut OperationEditArgs,
                fragment_start: &mut CharOffset,
                fragment_end: &mut CharOffset,
                range: Range<CharOffset>,
            ) -> bool {
                while let Some(fragment) = cursor.item() {
                    *fragment_start = *cursor.start();
                    *fragment_end = *fragment_start + fragment.visible_len(&self.undo_history);

                    // Check if range fully contains fragment.
                    if range.start < *fragment_start && range.end >= *fragment_end {
                        let mut new_fragment = fragment.clone();
                        new_fragment.mark_deletion(
                            operation_edit_args.lamport_timestamp.clone(),
                            &self.undo_history,
                        );
                        // Mark down the timestamp in our global map (used for operations such as
                        // undos later).
                        operation_edit_args
                            .version_in_range
                            .observe(&new_fragment.insertion.id);
                        // We still want to keep track of the fully deleted fragment in our
                        // new SumTree.
                        new_fragments.push(new_fragment);
                        // Move onto the next fragment.
                        cursor.next();
                        // If the range's end lines up with the current fragment's end,
                        // we're done looping and can update our edit operation.
                        if range.end == *fragment_end {
                            operation_edit_args.update_end(
                                fragment.insertion.id.clone(),
                                fragment.end_character_offset(),
                            );
                            // Indicates we're at a fragment boundary.
                            return true;
                        }
                    } else {
                        // Indicates we're not at a fragment boundary.
                        return false;
                    }
                }
                false
            }

            /// Style all fragments that are completely covered by the current `range`. Returns
            /// whether the range ends exactly at a fragment boundary.
            fn style_intermediate_fragments(
                &mut self,
                cursor: &mut Cursor<Fragment, CharOffset, CharOffset>,
                new_fragments: &mut SumTree<Fragment>,
                fragment_start: &mut CharOffset,
                fragment_end: &mut CharOffset,
                range: Range<CharOffset>,
                text_style_operation: TextStyleOperation,
            ) -> bool {
                while let Some(fragment) = cursor.item() {
                    *fragment_start = *cursor.start();
                    *fragment_end = *fragment_start + fragment.visible_len(&self.undo_history);

                    if range.start < *fragment_start && range.end >= *fragment_end {
                        let cur_text = fragment.text.clone();

                        let mut new_fragment = fragment.clone();
                        new_fragment.set_text_style(
                            TextStyleOperation::apply_text_style_operation(
                                cur_text.text_style.unwrap_or_default(),
                                text_style_operation,
                            ),
                        );
                        new_fragments.push(new_fragment.clone());
                        cursor.next();

                        if range.end == *fragment_end {
                            return true;
                        }
                    } else {
                        return false;
                    }
                }
                false
            }

            /// Searches through all the fragments on the buffer end, where the current
            /// fragment is to be inserted. If the fragments immediately to the on the buffer
            /// are deleted, this function returns the most recently deleted of this string
            /// of deleted fragments.
            fn last_deleted_fragment_at_buffer_end<'a>(
                &self,
                new_fragments: &'a SumTree<Fragment>,
            ) -> Option<&'a Fragment> {
                let summary = new_fragments.summary().text_summary.chars;

                let mut cursor = new_fragments.cursor::<CharOffset, CharOffset>();
                cursor.seek(&summary, SeekBias::Left);

                if cursor
                    .item()
                    .is_some_and(|f| f.is_visible(&self.undo_history))
                {
                    cursor.next();
                }
                let mut last_deleted_fragment_timestamp = None;
                let mut last_deleted_fragment = None;
                // Iterate rightwards to find the most recently deleted fragment
                for item in cursor {
                    if item.is_visible(&self.undo_history) {
                        break;
                    }
                    let max_timestamp = item.deletions.iter().max();
                    if max_timestamp > last_deleted_fragment_timestamp {
                        last_deleted_fragment_timestamp = max_timestamp;
                        last_deleted_fragment = Some(item);
                    }
                }

                last_deleted_fragment
            }

            /// Searches left of where the new fragment will be inserted in order to find
            /// the most immediate left visible fragment. Will return None if none exist.
            fn previous_visible_fragment_style(
                &self,
                new_fragments: &SumTree<Fragment>,
            ) -> Option<TextStyle> {
                let summary = new_fragments.summary().text_summary.chars;
                let mut cursor = new_fragments.cursor::<CharOffset, CharOffset>();

                cursor.seek(&summary, SeekBias::Left);
                while cursor
                    .item()
                    .is_some_and(|fragment| !fragment.is_visible(&self.undo_history))
                {
                    cursor.prev();
                }

                cursor.item().and_then(|fragment| {
                    fragment
                        .text
                        .text_style
                        .map(TextStyle::filter_inheritable_styles)
                })
            }

            /// Gets the text style from either the most recently deleted fragment to
            /// the right of the new fragment in the sum tree or, that fragment doesn't
            /// exist or it has already been inherited from, gets the style from the
            /// left visible fragment.
            fn style_from_left_visible_or_recently_deleted_fragment(
                &self,
                new_fragments: &mut SumTree<Fragment>,
            ) -> Option<TextStyle> {
                let last_deleted_fragment = self
                    .last_deleted_fragment_at_buffer_end(new_fragments)
                    .filter(|fragment| !fragment.text_style_inherited_after_deletion);

                match last_deleted_fragment {
                    None => self.previous_visible_fragment_style(new_fragments),
                    Some(last_deleted_fragment) => {
                        let (new_fragments_temp, text_style) = {
                            let mut new_last_deleted_fragment = last_deleted_fragment.clone();

                            // Slice up to the new_last_deleted_fragment
                            let mut cursor = new_fragments.cursor::<FragmentIdRef, CharOffset>();
                            let mut new_fragments_temp = cursor.slice(
                                &FragmentIdRef::new(&last_deleted_fragment.id),
                                SeekBias::Left,
                            );

                            let inherited_text_style = new_last_deleted_fragment
                                .text
                                .text_style
                                .map(TextStyle::filter_inheritable_styles);

                            // Mark the new_last_deleted_fragment as already inherited and add it to
                            // the SumTree
                            new_last_deleted_fragment.text_style_inherited_after_deletion = true;
                            new_fragments_temp.push(new_last_deleted_fragment);

                            // Move the cursor past the new_last_deleted_fragment then add the rest of the tree
                            // back after the new_fragments_temp
                            cursor.next();
                            new_fragments_temp.push_tree(
                                cursor.slice(
                                    &new_fragments.extent::<FragmentIdRef>(),
                                    SeekBias::Right,
                                ),
                            );
                            (new_fragments_temp, inherited_text_style)
                        };
                        *new_fragments = new_fragments_temp;
                        text_style
                    }
                }
            }

            fn build_fragment_to_insert(
                &mut self,
                prev_fragment: &Fragment,
                next_fragment: Option<&Fragment>,
                text: Text,
                lamport_timestamp: Lamport,
            ) -> Fragment {
                let next_fragment_id = match next_fragment {
                    Some(f) => &f.id,
                    None => FragmentId::max_value(),
                };
                let new_fragment_id = FragmentId::between(&prev_fragment.id, next_fragment_id);

                let mut split_tree = SumTree::new();
                split_tree.push(InsertionSplit {
                    extent: text.len(),
                    fragment_id: new_fragment_id.clone(),
                });
                self.insertion_splits
                    .insert(lamport_timestamp.clone(), split_tree);

                Fragment::new(
                    new_fragment_id,
                    Insertion {
                        id: lamport_timestamp,
                        parent_id: prev_fragment.insertion.id.clone(),
                        offset_in_parent: prev_fragment.end_character_offset(),
                        text,
                    },
                )
            }

            pub fn anchor_before<T: ToCharOffset>(&self, position: T) -> Result<Anchor> {
                self.anchor_at(position, AnchorBias::Left)
            }

            pub fn anchor_after<T: ToCharOffset>(&self, position: T) -> Result<Anchor> {
                self.anchor_at(position, AnchorBias::Right)
            }

            pub fn anchor_at<T: ToCharOffset>(
                &self,
                position: T,
                bias: AnchorBias,
            ) -> Result<Anchor> {
                let offset = position.to_char_offset(self)?;
                let max_offset = self.len();
                if offset > max_offset {
                    return Err(anyhow!("offset is out of range"));
                }

                let seek_bias;
                match bias {
                    AnchorBias::Left => {
                        if offset == 0.into() {
                            return Ok(Anchor::Start);
                        } else {
                            seek_bias = SeekBias::Left;
                        }
                    }
                    AnchorBias::Right => {
                        if offset == max_offset {
                            return Ok(Anchor::End);
                        } else {
                            seek_bias = SeekBias::Right;
                        }
                    }
                };

                let mut cursor = self.fragments.cursor::<CharOffset, CharOffset>();
                cursor.seek(&offset, seek_bias);
                let fragment = cursor.item().unwrap();
                let offset_in_fragment = offset - *cursor.start();
                let offset_in_insertion = fragment.start_character_offset() + offset_in_fragment;
                let anchor = Anchor::Middle {
                    insertion_id: fragment.insertion.id.clone(),
                    offset: offset_in_insertion,
                    bias,
                };
                Ok(anchor)
            }

            fn fragment_id_for_anchor(&self, anchor: &Anchor) -> Result<&FragmentId> {
                match anchor {
                    Anchor::Start => Ok(FragmentId::max_value()),
                    Anchor::End => Ok(FragmentId::min_value()),
                    Anchor::Middle {
                        insertion_id,
                        offset,
                        bias,
                        ..
                    } => {
                        let seek_bias = match bias {
                            AnchorBias::Left => SeekBias::Left,
                            AnchorBias::Right => SeekBias::Right,
                        };

                        let splits = self
                            .insertion_splits
                            .get(insertion_id)
                            .ok_or_else(|| anyhow!("split does not exist for insertion id"))?;
                        let mut splits_cursor = splits.cursor::<CharOffset, ()>();
                        splits_cursor.seek(offset, seek_bias);
                        splits_cursor
                            .item()
                            .ok_or_else(|| anyhow!("split offset is out of range"))
                            .map(|split| &split.fragment_id)
                    }
                }
            }

            fn summary_for_anchor(&self, anchor: &Anchor) -> Result<TextSummary> {
                match anchor {
                    Anchor::Start => Ok(TextSummary::default()),
                    Anchor::End => Ok(self.fragments.summary().text_summary),
                    Anchor::Middle {
                        insertion_id,
                        offset,
                        bias,
                    } => {
                        let seek_bias = match bias {
                            AnchorBias::Left => SeekBias::Left,
                            AnchorBias::Right => SeekBias::Right,
                        };

                        let splits = self
                            .insertion_splits
                            .get(insertion_id)
                            .ok_or_else(|| anyhow!("split does not exist for insertion id"))?;
                        let mut splits_cursor = splits.cursor::<CharOffset, ()>();
                        splits_cursor.seek(offset, seek_bias);
                        let split = splits_cursor
                            .item()
                            .ok_or_else(|| anyhow!("split offset is out of range"))?;

                        let mut fragments_cursor =
                            self.fragments.cursor::<FragmentIdRef, TextSummary>();
                        fragments_cursor
                            .seek(&FragmentIdRef::new(&split.fragment_id), SeekBias::Left);
                        let fragment = fragments_cursor
                            .item()
                            .ok_or_else(|| anyhow!("fragment id does not exist"))?;

                        let mut summary = fragments_cursor.start().clone();
                        if fragment.is_visible(&self.undo_history) {
                            summary += fragment
                                .text
                                .slice(..*offset - fragment.start_character_offset())
                                .summary();
                        }
                        Ok(summary)
                    }
                }
            }

            pub fn point_for_offset(&self, offset: impl ToCharOffset) -> Result<Point> {
                let offset = offset.to_char_offset(self)?;
                let mut fragments_cursor = self.fragments.cursor::<CharOffset, TextSummary>();
                fragments_cursor.seek(&offset, SeekBias::Left);
                fragments_cursor
                    .item()
                    .ok_or_else(|| anyhow!("offset is out of range"))
                    .map(|fragment| {
                        let overshoot =
                            fragment.point_for_offset(offset - fragments_cursor.start().chars);
                        fragments_cursor.start().lines + overshoot
                    })
            }

            fn merge_all_selections(&mut self) -> Result<()> {
                self.merge_local_selections()
            }

            /// Merges local selections only.
            fn merge_local_selections(&mut self) -> Result<()> {
                let mut local_selections = self.local_selections.selections.clone();
                self.merge_selections(&mut local_selections)?;
                self.local_selections.selections = local_selections;
                Ok(())
            }

            /// Merges any overlapping selections in-place in the given `selections` set.
            fn merge_selections<T>(&self, selections: &mut Vec1<T>) -> Result<()>
            where
                T: AsSelection,
            {
                let mut i = 1;

                while i < selections.len() {
                    if selections[i - 1]
                        .as_selection()
                        .end
                        .to_char_offset(self)
                        .unwrap()
                        >= selections[i]
                            .as_selection()
                            .start
                            .to_char_offset(self)
                            .unwrap()
                    {
                        let removed = selections.remove(i).unwrap();

                        if removed.as_selection().start.to_char_offset(self).unwrap()
                            < selections[i - 1]
                                .as_selection()
                                .start
                                .to_char_offset(self)
                                .unwrap()
                        {
                            selections[i - 1].as_mut_selection().start =
                                removed.as_selection().start.clone();
                        }

                        if removed.as_selection().end.to_char_offset(self).unwrap()
                            > selections[i - 1]
                                .as_selection()
                                .end
                                .to_char_offset(self)
                                .unwrap()
                        {
                            selections[i - 1].as_mut_selection().end =
                                removed.as_selection().end.clone();
                        }
                    } else {
                        i += 1;
                    }
                }

                Ok(())
            }

            pub fn marked_text_state(&self) -> MarkedTextState {
                self.local_selections.marked_text_state()
            }

            pub fn set_marked_text_state(&mut self, marked_text_state: MarkedTextState) {
                self.local_selections
                    .set_marked_text_state(marked_text_state);
            }
        }

        impl TextBuffer for Buffer {
            type Chars<'a>
                = Chars<'a>
            where
                Self: 'a;

            type CharsReverse<'a>
                = Chars<'a>
            where
                Self: 'a;

            fn chars_at(&self, offset: CharOffset) -> Result<Self::Chars<'_>> {
                Buffer::chars_at(self, offset)
            }

            fn chars_rev_at(&self, offset: CharOffset) -> Result<Self::CharsReverse<'_>> {
                Buffer::chars_at(self, offset).map(|chars| chars.rev())
            }

            fn to_point(&self, offset: CharOffset) -> Result<Point> {
                offset.to_point(self)
            }

            fn to_offset(&self, point: Point) -> Result<CharOffset> {
                <Point as ToCharOffset>::to_char_offset(&point, self)
            }
        }

        #[derive(Clone, Debug, Eq, PartialEq)]
        pub enum Event {
            /// A local edit was made.
            /// There may have also been selection changes.
            Edited {
                /// The edited ranges.
                edits: Vec<Edit>,
                /// The origin of the edit.
                edit_origin: EditOrigin,
            },

            /// Selection state was changed.
            /// If the selection change was associated to
            /// an edit, then we rely on the [`Self::Edited`]
            /// event.
            SelectionsChanged,

            /// Styles were updated.
            StylesUpdated,
        }

        impl Entity for Buffer {
            type Event = Event;
        }

        impl sum_tree::Dimension<'_, FragmentSummary> for Point {
            fn add_summary(&mut self, summary: &FragmentSummary) {
                *self += summary.text_summary.lines;
            }
        }

        impl CharsWithStyle<'_> {
            pub fn rev(mut self) -> Self {
                if self.reversed {
                    self.reversed = false;
                    self.fragment_chars =
                        self.fragments_cursor.item().map_or("".chars(), |fragment| {
                            fragment.text[self.fragment_offset..].chars()
                        });
                } else {
                    self.reversed = true;
                    self.fragment_chars =
                        self.fragments_cursor.item().map_or("".chars(), |fragment| {
                            fragment.text[..self.fragment_offset].chars()
                        });
                }

                self
            }
        }

        impl Chars<'_> {
            pub fn rev(self) -> Self {
                Self(self.0.rev())
            }
        }

        impl Iterator for Chars<'_> {
            type Item = char;

            fn next(&mut self) -> Option<Self::Item> {
                self.0.next().map(Into::into)
            }
        }

        impl Iterator for TextStyleRuns<'_> {
            type Item = TextRun;

            fn next(&mut self) -> Option<Self::Item> {
                let mut current_string = String::new();
                let mut current_text_style = None;

                while let Some(fragment) = self.fragments_cursor.item() {
                    if fragment.is_visible(self.undo_history) && !fragment.text.as_str().is_empty()
                    {
                        match current_text_style {
                            None => {
                                current_text_style =
                                    Some(fragment.text.text_style().unwrap_or_default());
                                current_string.push_str(fragment.text.as_str());
                                self.byte_index_start = *self.fragments_cursor.start();
                                self.byte_index_end =
                                    *self.fragments_cursor.start() + fragment.text.byte_len();
                            }
                            Some(text_style) => {
                                let fragment_text_style =
                                    fragment.text.text_style().unwrap_or_default();

                                // If the text style is the same as the last fragment, merge the strings
                                // together.
                                if text_style == fragment_text_style {
                                    current_string.push_str(fragment.text.as_str());
                                    self.byte_index_end += fragment.text.as_str().len();
                                } else {
                                    return Some(TextRun::new(
                                        current_string,
                                        text_style,
                                        self.byte_index_start..self.byte_index_end,
                                    ));
                                }
                            }
                        }
                    }
                    self.fragments_cursor.next();
                }

                current_text_style.map(|text_style| {
                    TextRun::new(
                        current_string,
                        text_style,
                        self.byte_index_start..self.byte_index_end,
                    )
                })
            }
        }

        impl Iterator for CharsWithStyle<'_> {
            type Item = StylizedChar;

            fn next(&mut self) -> Option<Self::Item> {
                if self.reversed {
                    if let Some(char) = self.fragment_chars.next_back() {
                        self.fragment_offset -= 1;
                        Some(StylizedChar::new(char, self.fragment_text_style))
                    } else {
                        loop {
                            self.fragments_cursor.prev();
                            let fragment = self.fragments_cursor.item()?;
                            if fragment.is_visible(self.undo_history) {
                                self.fragment_offset = fragment.len();
                                self.fragment_chars = fragment.text.as_str().chars();
                                self.fragment_text_style =
                                    fragment.text.text_style().unwrap_or_default();
                                return self
                                    .fragment_chars
                                    .next_back()
                                    .map(|char| StylizedChar::new(char, self.fragment_text_style));
                            }
                        }
                    }
                } else if let Some(char) = self.fragment_chars.next() {
                    self.fragment_offset += 1;
                    Some(StylizedChar::new(char, self.fragment_text_style))
                } else {
                    loop {
                        self.fragments_cursor.next();
                        let fragment = self.fragments_cursor.item()?;
                        if fragment.is_visible(self.undo_history) {
                            self.fragment_chars = fragment.text.as_str().chars();
                            self.fragment_text_style =
                                fragment.text.text_style().unwrap_or_default();
                            self.fragment_offset = 0.into();
                            return self
                                .fragment_chars
                                .next()
                                .map(|char| StylizedChar::new(char, self.fragment_text_style));
                        }
                    }
                }
            }
        }

        impl<F: Fn(&FragmentSummary) -> bool> Iterator for Edits<'_, F> {
            type Item = Edit;

            fn next(&mut self) -> Option<Self::Item> {
                let mut change: Option<Edit> = None;

                while let Some(fragment) = self.cursor.item() {
                    let new_offset = *self.cursor.start();
                    let old_offset =
                        CharOffset::from((new_offset.as_usize() as isize - self.delta) as usize);

                    if !fragment.was_visible(&self.since, self.undo_history)
                        && fragment.is_visible(self.undo_history)
                    {
                        if let Some(ref mut change) = change {
                            if change.new_range.end == new_offset {
                                change.new_range.end += fragment.len();
                                self.delta += fragment.len().as_usize() as isize;
                            } else {
                                break;
                            }
                        } else {
                            change = Some(Edit {
                                old_range: old_offset..old_offset,
                                new_range: new_offset..new_offset + fragment.len(),
                            });
                            self.delta += fragment.len().as_usize() as isize;
                        }
                    } else if fragment.was_visible(&self.since, self.undo_history)
                        && !fragment.is_visible(self.undo_history)
                    {
                        if let Some(ref mut change) = change {
                            if change.new_range.end == new_offset {
                                change.old_range.end += fragment.len();
                                self.delta -= fragment.len().as_usize() as isize;
                            } else {
                                break;
                            }
                        } else {
                            change = Some(Edit {
                                old_range: old_offset..old_offset + fragment.len(),
                                new_range: new_offset..new_offset,
                            });
                            self.delta -= fragment.len().as_usize() as isize;
                        }
                    }

                    self.cursor.next();
                }

                change
            }
        }

        #[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Debug)]
        struct FragmentId(Box<[u16]>);

        lazy_static! {
            static ref FRAGMENT_ID_EMPTY: FragmentId = FragmentId(Box::new([]));
            static ref FRAGMENT_ID_MIN_VALUE: FragmentId = FragmentId(Box::new([0]));
            static ref FRAGMENT_ID_MAX_VALUE: FragmentId = FragmentId(Box::new([u16::MAX]));
        }

        impl Default for FragmentId {
            fn default() -> Self {
                FRAGMENT_ID_EMPTY.clone()
            }
        }

        impl FragmentId {
            fn min_value() -> &'static Self {
                &FRAGMENT_ID_MIN_VALUE
            }

            fn max_value() -> &'static Self {
                &FRAGMENT_ID_MAX_VALUE
            }

            fn between(left: &Self, right: &Self) -> Self {
                Self::between_with_max(left, right, u16::MAX)
            }

            fn between_with_max(left: &Self, right: &Self, max_value: u16) -> Self {
                let mut new_entries = Vec::new();

                let left_entries = left.0.iter().cloned().chain(iter::repeat(0));
                let right_entries = right.0.iter().cloned().chain(iter::repeat(max_value));
                for (l, r) in left_entries.zip(right_entries) {
                    let interval = r - l;
                    if interval > 1 {
                        new_entries.push(l + (interval / 2).clamp(1, 8));
                        break;
                    } else {
                        new_entries.push(l);
                    }
                }

                FragmentId(new_entries.into_boxed_slice())
            }
        }

        #[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Debug, Default)]
        struct FragmentIdRef<'a>(Option<&'a FragmentId>);

        impl<'a> FragmentIdRef<'a> {
            fn new(id: &'a FragmentId) -> Self {
                Self(Some(id))
            }
        }

        impl<'a> sum_tree::Dimension<'a, FragmentSummary> for FragmentIdRef<'a> {
            fn add_summary(&mut self, summary: &'a FragmentSummary) {
                self.0 = Some(&summary.max_fragment_id)
            }
        }

        impl Fragment {
            fn new(id: FragmentId, insertion: Insertion) -> Self {
                Self {
                    id,
                    text: insertion.text.clone(),
                    insertion,
                    deletions: BTreeSet::new(),
                    text_style_inherited_after_deletion: false,
                    is_visible: true,
                    max_undo_ts: Global::new(),
                }
            }

            fn mark_deletion(&mut self, deletion: Lamport, undo_history: &UndoHistory) {
                self.deletions.insert(deletion);
                self.recompute_visibility(undo_history);
            }

            fn set_text_style(&mut self, text_style: TextStyle) {
                self.text.text_style = Some(text_style);
            }

            /// The number of characters within the underlying `Text` the fragment starts.
            fn start_character_offset(&self) -> CharOffset {
                self.text.range().start
            }

            fn set_start_offset(&mut self, offset: CharOffset) {
                self.text = self
                    .insertion
                    .text
                    .slice(offset..self.end_character_offset())
                    .with_text_style(self.text.text_style());
            }

            /// The number of characters within the underlying `Text` the fragment ends.
            fn end_character_offset(&self) -> CharOffset {
                self.text.range().end
            }

            fn set_end_character_offset(&mut self, offset: CharOffset) {
                self.text = self
                    .insertion
                    .text
                    .slice(self.start_character_offset()..offset)
                    .with_text_style(self.text.text_style());
            }

            fn visible_len(&self, undo_history: &UndoHistory) -> CharOffset {
                if self.is_visible(undo_history) {
                    self.len()
                } else {
                    0.into()
                }
            }

            fn len(&self) -> CharOffset {
                self.text.len()
            }

            fn recompute_visibility(&mut self, undo_history: &UndoHistory) {
                self.is_visible = self.is_visible(undo_history);
            }

            /// Returns true iff the fragment's edit should be incorporated
            /// into the final buffer.
            ///
            /// Prefer using the [`Self::is_visible`] function over
            /// the `is_visible` attribute to query the freshest state.
            fn is_visible(&self, undo_history: &UndoHistory) -> bool {
                !undo_history.is_edit_undone(&self.insertion.id)
                    && self
                        .deletions
                        .iter()
                        .all(|deletion| undo_history.is_edit_undone(deletion))
            }

            /// Returns true iff the fragment's edit should have been incorporated
            /// when the buffer's version vector was `version`.
            fn was_visible(&self, version: &Global, undo_history: &UndoHistory) -> bool {
                version.observed(&self.insertion.id)
                    && !undo_history.was_edit_undone(&self.insertion.id, version)
                    && self
                        .deletions
                        .iter()
                        .all(|d| !version.observed(d) || undo_history.was_edit_undone(d, version))
            }

            fn point_for_offset(&self, offset: CharOffset) -> Point {
                self.text.point_for_offset(offset)
            }

            fn offset_for_point(&self, point: Point) -> CharOffset {
                self.text.offset_for_point(point)
            }

            fn byte_offset_for_point(&self, point: Point) -> ByteOffset {
                self.text.byte_offset_for_point(point)
            }

            /// Converts a `ByteOffset` from the start of the fragment to its corresponding `CharOffset`.
            fn char_offset_for_byte_offset(&self, byte_offset: ByteOffset) -> CharOffset {
                self.text.char_offset_for_byte_offset(byte_offset)
            }
        }

        impl sum_tree::Item for Fragment {
            type Summary = FragmentSummary;

            fn summary(&self) -> Self::Summary {
                let mut max_version = Global::new();
                max_version.observe(&self.insertion.id);
                for deletion in &self.deletions {
                    max_version.observe(deletion);
                }
                max_version.observe_all(&self.max_undo_ts);

                if self.is_visible {
                    FragmentSummary {
                        text_summary: self.text.summary(),
                        max_fragment_id: self.id.clone(),
                        max_version,
                    }
                } else {
                    FragmentSummary {
                        text_summary: TextSummary::default(),
                        max_fragment_id: self.id.clone(),
                        max_version,
                    }
                }
            }
        }

        impl AddAssign<&FragmentSummary> for FragmentSummary {
            fn add_assign(&mut self, other: &Self) {
                self.text_summary += &other.text_summary;
                debug_assert!(self.max_fragment_id <= other.max_fragment_id);
                self.max_fragment_id = other.max_fragment_id.clone();
                self.max_version.observe_all(&other.max_version);
            }
        }

        impl Default for FragmentSummary {
            fn default() -> Self {
                FragmentSummary {
                    text_summary: TextSummary::default(),
                    max_fragment_id: FragmentId::min_value().clone(),
                    max_version: Global::new(),
                }
            }
        }

        impl sum_tree::Dimension<'_, FragmentSummary> for TextSummary {
            fn add_summary(&mut self, summary: &FragmentSummary) {
                *self += &summary.text_summary;
            }
        }

        impl sum_tree::Dimension<'_, FragmentSummary> for CharOffset {
            fn add_summary(&mut self, summary: &FragmentSummary) {
                *self += summary.text_summary.chars;
            }
        }

        impl sum_tree::Dimension<'_, FragmentSummary> for ByteOffset {
            fn add_summary(&mut self, summary: &FragmentSummary) {
                *self += summary.text_summary.bytes;
            }
        }

        impl sum_tree::Item for InsertionSplit {
            type Summary = InsertionSplitSummary;

            fn summary(&self) -> Self::Summary {
                InsertionSplitSummary {
                    extent: self.extent,
                }
            }
        }

        impl AddAssign<&InsertionSplitSummary> for InsertionSplitSummary {
            fn add_assign(&mut self, other: &Self) {
                self.extent += other.extent;
            }
        }

        impl sum_tree::Dimension<'_, InsertionSplitSummary> for CharOffset {
            fn add_summary(&mut self, summary: &InsertionSplitSummary) {
                *self += summary.extent;
            }
        }

        pub trait ToCharOffset {
            fn to_char_offset(&self, buffer: &Buffer) -> Result<CharOffset>;
        }

        pub trait ToBufferOffset {
            fn to_byte_offset(&self, buffer: &Buffer) -> Result<ByteOffset>;
        }

        impl ToCharOffset for Point {
            fn to_char_offset(&self, buffer: &Buffer) -> Result<CharOffset> {
                let mut fragments_cursor = buffer.fragments.cursor::<Point, TextSummary>();
                fragments_cursor.seek(self, SeekBias::Left);
                fragments_cursor
                    .item()
                    .ok_or_else(|| anyhow!("point is out of range"))
                    .map(|fragment| {
                        let overshoot =
                            fragment.offset_for_point(*self - fragments_cursor.start().lines);
                        fragments_cursor.start().chars + overshoot
                    })
            }
        }

        impl ToCharOffset for CharOffset {
            fn to_char_offset(&self, _: &Buffer) -> Result<CharOffset> {
                Ok(*self)
            }
        }

        impl ToCharOffset for Anchor {
            fn to_char_offset(&self, buffer: &Buffer) -> Result<CharOffset> {
                Ok(buffer.summary_for_anchor(self)?.chars)
            }
        }

        impl ToCharOffset for &Anchor {
            fn to_char_offset(&self, buffer: &Buffer) -> Result<CharOffset> {
                Ok(buffer.summary_for_anchor(self)?.chars)
            }
        }

        impl ToCharOffset for ByteOffset {
            fn to_char_offset(&self, buffer: &Buffer) -> Result<CharOffset> {
                let mut fragments_cursor = buffer.fragments.cursor::<ByteOffset, TextSummary>();
                fragments_cursor.seek(self, SeekBias::Left);
                fragments_cursor
                    .item()
                    .ok_or_else(|| anyhow!("byte index is out of range"))
                    .map(|fragment| {
                        let chars_to_start_of_fragment = fragments_cursor.start().chars;

                        // Compute the number of bytes past the start of the fragment, and call into the
                        // fragment to convert the number of bytes into a character offset.
                        let num_bytes_overshot = *self - fragments_cursor.start().bytes;
                        chars_to_start_of_fragment
                            + fragment.char_offset_for_byte_offset(num_bytes_overshot)
                    })
            }
        }

        impl ToBufferOffset for Point {
            fn to_byte_offset(&self, buffer: &Buffer) -> Result<ByteOffset> {
                let mut fragments_cursor = buffer.fragments.cursor::<Point, TextSummary>();
                fragments_cursor.seek(self, SeekBias::Left);
                fragments_cursor
                    .item()
                    .ok_or_else(|| anyhow!("point is out of range"))
                    .map(|fragment| {
                        let overshoot =
                            fragment.byte_offset_for_point(*self - fragments_cursor.start().lines);
                        fragments_cursor.start().bytes + overshoot
                    })
            }
        }

        impl ToBufferOffset for ByteOffset {
            fn to_byte_offset(&self, _: &Buffer) -> Result<ByteOffset> {
                Ok(*self)
            }
        }

        impl ToBufferOffset for Anchor {
            fn to_byte_offset(&self, buffer: &Buffer) -> Result<ByteOffset> {
                Ok(buffer.summary_for_anchor(self)?.bytes)
            }
        }

        impl ToBufferOffset for &Anchor {
            fn to_byte_offset(&self, buffer: &Buffer) -> Result<ByteOffset> {
                Ok(buffer.summary_for_anchor(self)?.bytes)
            }
        }

        impl ToBufferOffset for CharOffset {
            fn to_byte_offset(&self, buffer: &Buffer) -> Result<ByteOffset> {
                let point = buffer.point_for_offset(*self)?;
                point.to_byte_offset(buffer)
            }
        }

        pub trait ToPoint {
            fn to_point(&self, buffer: &Buffer) -> Result<Point>;
        }

        impl ToPoint for Anchor {
            fn to_point(&self, buffer: &Buffer) -> Result<Point> {
                Ok(buffer.summary_for_anchor(self)?.lines)
            }
        }

        impl ToPoint for CharOffset {
            fn to_point(&self, buffer: &Buffer) -> Result<Point> {
                let mut fragments_cursor = buffer.fragments.cursor::<CharOffset, TextSummary>();
                fragments_cursor.seek(self, SeekBias::Left);
                fragments_cursor
                    .item()
                    .ok_or_else(|| anyhow!("offset is out of range"))
                    .map(|fragment| {
                        let overshoot =
                            fragment.point_for_offset(*self - fragments_cursor.start().chars);
                        fragments_cursor.start().lines + overshoot
                    })
            }
        }

        mod anchor {
            use std::cmp::Ordering;
            use std::ops::Range;

            use anyhow::Result;
            use serde::{Deserialize, Serialize};
            use string_offset::CharOffset;
            use time::Lamport;

            use super::{Buffer, time};

            #[derive(Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize)]
            pub enum Anchor {
                Start,
                End,
                Middle {
                    insertion_id: Lamport,
                    offset: CharOffset,
                    bias: AnchorBias,
                },
            }

            #[derive(Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize)]
            pub enum AnchorBias {
                Left,
                Right,
            }

            impl PartialOrd for AnchorBias {
                fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                    Some(self.cmp(other))
                }
            }

            impl Ord for AnchorBias {
                fn cmp(&self, other: &Self) -> Ordering {
                    use AnchorBias::*;

                    if self == other {
                        return Ordering::Equal;
                    }

                    match (self, other) {
                        (Left, _) => Ordering::Less,
                        (Right, _) => Ordering::Greater,
                    }
                }
            }

            impl Anchor {
                pub fn cmp(&self, other: &Anchor, buffer: &Buffer) -> Result<Ordering> {
                    if self == other {
                        return Ok(Ordering::Equal);
                    }

                    Ok(match (self, other) {
                        (Anchor::Start, _) | (_, Anchor::End) => Ordering::Less,
                        (Anchor::End, _) | (_, Anchor::Start) => Ordering::Greater,
                        (
                            Anchor::Middle {
                                offset: self_offset,
                                bias: self_bias,
                                ..
                            },
                            Anchor::Middle {
                                offset: other_offset,
                                bias: other_bias,
                                ..
                            },
                        ) => buffer
                            .fragment_id_for_anchor(self)?
                            .cmp(buffer.fragment_id_for_anchor(other)?)
                            .then_with(|| self_offset.cmp(other_offset))
                            .then_with(|| self_bias.cmp(other_bias)),
                    })
                }
            }

            pub trait AnchorRangeExt {
                fn cmp(&self, b: &Range<Anchor>, buffer: &Buffer) -> Result<Ordering>;
            }

            impl AnchorRangeExt for Range<Anchor> {
                fn cmp(&self, other: &Range<Anchor>, buffer: &Buffer) -> Result<Ordering> {
                    Ok(match self.start.cmp(&other.start, buffer)? {
                        Ordering::Equal => other.end.cmp(&self.end, buffer)?,
                        ord => ord,
                    })
                }
            }
        }

        mod subword_boundaries {
            use std::iter::Peekable;

            use itertools::Either;
            use warpui::text::TextBuffer;
            use warpui::text::word_boundaries::WordBoundariesApproach;
            use warpui::text::words::is_subword_boundary_char;

            use super::{CharOffset, Point};

            pub struct SubwordBoundaries<'a, T: TextBuffer + ?Sized> {
                offset: CharOffset,
                chars: Peekable<Either<T::Chars<'a>, T::CharsReverse<'a>>>,
                char_window: CharWindow,
                buffer: &'a T,
                approach: WordBoundariesApproach,
                in_word: bool,
                done: bool,
            }

            impl<'a, T: TextBuffer + ?Sized> SubwordBoundaries<'a, T> {
                /// Returns an iterator for the beginnings of the subwords in the buffer.
                pub fn forward_subword_starts(
                    offset: CharOffset,
                    chars: T::Chars<'a>,
                    buffer: &'a T,
                ) -> Self {
                    let mut peekable_chars = Either::Left(chars).peekable();
                    let first = peekable_chars.next();
                    let second = peekable_chars.next();
                    let third = peekable_chars.next();
                    Self {
                        offset,
                        buffer,
                        chars: peekable_chars,
                        char_window: CharWindow::new(first, second, third),
                        in_word: true,
                        approach: WordBoundariesApproach::ForwardWordStarts,
                        done: false,
                    }
                }

                /// Returns an iterator for the ends of the subwords in the buffer.
                pub fn forward_subword_ends_exclusive(
                    offset: CharOffset,
                    chars: T::Chars<'a>,
                    buffer: &'a T,
                ) -> Self {
                    let mut peekable_chars = Either::Left(chars).peekable();
                    let first = peekable_chars.next();
                    let second = peekable_chars.next();
                    let third = peekable_chars.next();
                    Self {
                        offset,
                        buffer,
                        chars: peekable_chars,
                        char_window: CharWindow::new(first, second, third),
                        in_word: false,
                        approach: WordBoundariesApproach::ForwardWordEnds,
                        done: false,
                    }
                }

                /// Returns a backwards iterator for the beginning of subwords in the buffer.
                pub fn backward_subword_starts_exclusive(
                    offset: CharOffset,
                    chars: T::CharsReverse<'a>,
                    buffer: &'a T,
                ) -> Self {
                    let mut peekable_chars = Either::Right(chars).peekable();
                    let first = peekable_chars.next();
                    let second = peekable_chars.next();
                    let third = peekable_chars.next();
                    Self {
                        offset,
                        buffer,
                        chars: peekable_chars,
                        char_window: CharWindow::new(first, second, third),
                        in_word: false,
                        approach: WordBoundariesApproach::BackwardWordStarts,
                        done: false,
                    }
                }

                /// Move to the next character and update the offset.
                fn step(&mut self) {
                    let new_char = self.chars.next();
                    self.char_window.forward(new_char.to_owned());

                    match self.approach {
                        WordBoundariesApproach::ForwardWordStarts
                        | WordBoundariesApproach::ForwardWordEnds => {
                            self.offset += 1;
                        }
                        WordBoundariesApproach::BackwardWordStarts => {
                            self.offset -= 1;
                        }
                    }
                }

                /// Helper for the `Iterator::next()` implementation.
                /// Moves forward through the string and returns the start of the next
                /// subword.
                fn next_forward_starts(&mut self) -> Option<<Self as Iterator>::Item> {
                    while let Some(c) = self.char_window.first() {
                        if self.in_word {
                            // We're in a word, but are we in a subword?
                            if is_subword_boundary_char(c) {
                                // c isn't part of a subword.
                                self.in_word = false;
                                self.step();
                            } else if c.is_uppercase() {
                                if let Some(c_next) = self.char_window.second() {
                                    // If c is upper and c_next is lower, then
                                    // c marks the start of a Capitalized word.
                                    if c_next.is_lowercase() {
                                        let point = self.buffer.to_point(self.offset).ok();
                                        self.step(); // to avoid getting stuck
                                        return point;
                                    } else if c_next.is_uppercase() {
                                        // peek once more, to see if c is the "X" in "XYz"
                                        if let Some(_c_next_next) = self.char_window.third() {
                                            self.step();
                                        } else {
                                            // we've reached the last character in the string,
                                            // and since it's uppercase it should be treated
                                            // as the start of a one-letter word.
                                            self.step();
                                            return self.buffer.to_point(self.offset).ok();
                                        }
                                    } else {
                                        self.step();
                                    }
                                } else {
                                    // c_next wasn't available.
                                    self.step();
                                }
                            } else if c.is_lowercase() {
                                self.step();

                                if let Some(c_next) = self.char_window.first()
                                    && c_next.is_uppercase()
                                {
                                    let point = self.buffer.to_point(self.offset).ok();
                                    self.step();
                                    return point;
                                }
                            } else {
                                self.step();
                            }

                        // Still haven't entered a word.
                        } else if is_subword_boundary_char(c) {
                            self.step();

                        // Just entered a word.
                        } else {
                            self.in_word = true;
                            let point = self.buffer.to_point(self.offset).ok();
                            self.step();
                            return point;
                        }
                    }
                    None
                }

                /// Helper for the `Iterator::next()` implementation.
                /// Moves forward through the string and returns the end of the next
                /// subword.
                fn next_forward_ends(&mut self) -> Option<<Self as Iterator>::Item> {
                    while let Some(c) = self.char_window.first() {
                        if self.in_word && is_subword_boundary_char(c) {
                            // We are in a word, but the next character is _not_ in a word,
                            // so we have found the boundary.
                            self.in_word = false;
                            return self.buffer.to_point(self.offset).ok();
                        }

                        if !self.in_word && !is_subword_boundary_char(c) {
                            self.in_word = true;
                        }

                        if self.in_word
                            && let Some(c_next) = self.char_window.second()
                        {
                            if c.is_lowercase() && c_next.is_uppercase() {
                                // c is the end of a word, and c_next begins the next word.
                                self.step();
                                return self.buffer.to_point(self.offset).ok();
                            } else if c.is_uppercase() && c_next.is_uppercase() {
                                // c_next could be part of an all-caps word
                                // or the start of a new Capitalized word
                                if let Some(c_next_next) = self.char_window.third()
                                    && c_next_next.is_lowercase()
                                {
                                    self.step();
                                    return self.buffer.to_point(self.offset).ok();
                                }
                            }
                        }

                        self.step();
                    }
                    None
                }

                /// Helper for the `Iterator::next()` implementation.
                /// Moves backward through the string and returns the start of the closest
                /// subword.
                fn next_backward_starts(&mut self) -> Option<<Self as Iterator>::Item> {
                    while let Some(c) = self.char_window.first() {
                        if self.in_word && is_subword_boundary_char(c) {
                            self.in_word = false;
                            let point = self.buffer.to_point(self.offset).ok();
                            self.step();
                            return point;
                        }

                        if !self.in_word && !is_subword_boundary_char(c) {
                            self.in_word = true;
                        }

                        if self.in_word
                            && let Some(c_next) = self.char_window.second()
                        {
                            if c.is_lowercase() && c_next.is_uppercase() {
                                // c_next is the start of the c's subword.
                                self.step();
                                self.step();
                                let point = self.buffer.to_point(self.offset).ok();
                                self.step(); // to avoid re-designating this character as a start
                                return point;
                            } else if c.is_uppercase() && c_next.is_lowercase() {
                                // c is the start Capitalized or Uppercase subword,
                                // and c_next is the end of a Capitalized or lowercase
                                // subword to the right of it.
                                self.step();
                                let point = self.buffer.to_point(self.offset).ok();
                                return point;
                            }
                        }

                        self.step();
                    }
                    None
                }
            }

            impl<T: TextBuffer + ?Sized> Iterator for SubwordBoundaries<'_, T> {
                type Item = Point;

                fn next(&mut self) -> Option<Self::Item> {
                    if let Some(point) = match self.approach {
                        WordBoundariesApproach::ForwardWordStarts => self.next_forward_starts(),
                        WordBoundariesApproach::ForwardWordEnds => self.next_forward_ends(),
                        WordBoundariesApproach::BackwardWordStarts => self.next_backward_starts(),
                    } {
                        return Some(point);
                    }

                    // We have consumed all of the characters in the given direction. However, we should also
                    // treat the end (or beginning if backward) of the buffer as a word boundary. We only want
                    // to return that once, however, so we mark ourselves as done afterwards.
                    if self.done {
                        None
                    } else {
                        self.done = true;

                        self.buffer.to_point(self.offset).ok()
                    }
                }
            }

            /// Storage for characters from the buffer, used by the `SubwordBoundaries`
            /// iterator to find the start and end of subwords.
            struct CharWindow {
                /// A store of characters retrieved from the `chars` iterator.
                ///
                /// `char_window[0]`: character at the current offset.
                ///
                /// `char_window[1]`: character after the current offset.
                ///
                /// `char_window[2]`: two characters after the current offset.
                window: Vec<Option<char>>,
            }

            impl CharWindow {
                fn new(first: Option<char>, second: Option<char>, third: Option<char>) -> Self {
                    Self {
                        window: vec![first, second, third],
                    }
                }

                fn forward(&mut self, new_char: Option<char>) {
                    self.window.remove(0);
                    self.window.push(new_char);
                }

                fn first(&self) -> Option<char> {
                    self.window.first().unwrap_or(&None).to_owned()
                }

                fn second(&self) -> Option<char> {
                    self.window.get(1).unwrap_or(&None).to_owned()
                }

                fn third(&self) -> Option<char> {
                    self.window.get(2).unwrap_or(&None).to_owned()
                }
            }
        }

        mod text {
            use std::cmp;
            use std::fmt::{self, Debug};
            use std::ops::{Bound, Index, Range, RangeBounds};
            use std::rc::Rc;

            use arrayvec::ArrayVec;
            use num_traits::SaturatingSub;
            use string_offset::{ByteOffset, CharOffset};
            use sum_tree::{self, SeekBias, SumTree};
            use warpui::text::point::Point;
            use warpui::text_layout::TextStyle;

            #[derive(Copy, Clone, Debug, Eq, PartialEq)]
            enum Run {
                Newline,
                Chars { len: usize, char_size: u8 },
            }

            impl sum_tree::Item for Run {
                type Summary = TextSummary;

                fn summary(&self) -> Self::Summary {
                    match *self {
                        Run::Newline => TextSummary {
                            chars: 1.into(),
                            bytes: 1.into(),
                            lines: Point::new(1, 0),
                            first_line_len: 0,
                            rightmost_point: Point::new(0, 0),
                        },
                        Run::Chars { len, char_size } => TextSummary {
                            chars: len.into(),
                            bytes: (len * char_size as usize).into(),
                            lines: Point::new(0, len as u32),
                            first_line_len: len as u32,
                            rightmost_point: Point::new(0, len as u32),
                        },
                    }
                }
            }

            impl Run {
                fn char_size(&self) -> u8 {
                    match self {
                        Run::Newline => 1,
                        Run::Chars { char_size, .. } => *char_size,
                    }
                }
            }

            /// A summary of text locations.
            #[derive(Clone, Debug, Default, Eq, PartialEq)]
            pub struct TextSummary {
                pub chars: CharOffset,
                pub bytes: ByteOffset,
                pub lines: Point,
                pub first_line_len: u32,
                pub rightmost_point: Point,
            }

            impl<'a> std::ops::AddAssign<&'a Self> for TextSummary {
                fn add_assign(&mut self, other: &'a Self) {
                    let joined_line_len = self.lines.column + other.first_line_len;
                    if joined_line_len > self.rightmost_point.column {
                        self.rightmost_point = Point::new(self.lines.row, joined_line_len);
                    }
                    if other.rightmost_point.column > self.rightmost_point.column {
                        self.rightmost_point = self.lines + other.rightmost_point;
                    }

                    if self.lines.row == 0 {
                        self.first_line_len += other.first_line_len;
                    }

                    self.chars += other.chars;
                    self.bytes += other.bytes;
                    self.lines += other.lines;
                }
            }

            impl std::ops::AddAssign<Self> for TextSummary {
                fn add_assign(&mut self, other: Self) {
                    *self += &other;
                }
            }

            impl sum_tree::Dimension<'_, TextSummary> for TextSummary {
                fn add_summary(&mut self, summary: &TextSummary) {
                    *self += summary;
                }
            }

            impl sum_tree::Dimension<'_, TextSummary> for Point {
                fn add_summary(&mut self, summary: &TextSummary) {
                    *self += summary.lines;
                }
            }

            impl sum_tree::Dimension<'_, TextSummary> for ByteOffset {
                fn add_summary(&mut self, summary: &TextSummary) {
                    *self += summary.bytes
                }
            }

            impl sum_tree::Dimension<'_, TextSummary> for CharOffset {
                fn add_summary(&mut self, summary: &TextSummary) {
                    *self += summary.chars;
                }
            }

            #[derive(Clone)]
            pub struct Text {
                text: Rc<str>,
                runs: SumTree<Run>,
                range: Range<CharOffset>,
                pub text_style: Option<TextStyle>,
            }

            impl Text {
                pub fn new(text: impl Into<String>, text_style: Option<TextStyle>) -> Self {
                    let mut text = Text::from(text.into());
                    text.text_style = text_style;
                    text
                }

                pub fn with_text_style(mut self, text_style: impl Into<Option<TextStyle>>) -> Self {
                    self.text_style = text_style.into();
                    self
                }

                pub fn fallback_text_style_with<F>(&mut self, fallback: F)
                where
                    F: FnOnce() -> Option<TextStyle>,
                {
                    if self.text_style.is_none() {
                        self.text_style = fallback();
                    }
                }

                pub fn text_style(&self) -> Option<TextStyle> {
                    self.text_style
                }
            }

            impl From<String> for Text {
                fn from(text: String) -> Self {
                    let mut runs = Vec::new();

                    let mut chars_len = 0;
                    let mut run_char_size = 0;
                    let mut run_chars = 0;

                    let mut chars = text.chars();
                    loop {
                        let ch = chars.next();
                        let ch_size = ch.map_or(0, |ch| ch.len_utf8());
                        if run_chars != 0
                            && (ch.is_none() || ch == Some('\n') || run_char_size != ch_size)
                        {
                            runs.push(Run::Chars {
                                len: run_chars,
                                char_size: run_char_size as u8,
                            });
                            run_chars = 0;
                        }
                        run_char_size = ch_size;

                        match ch {
                            Some('\n') => runs.push(Run::Newline),
                            Some(_) => run_chars += 1,
                            None => break,
                        }
                        chars_len += 1;
                    }

                    let mut tree = SumTree::new();
                    tree.extend(runs);
                    Text {
                        text: text.into(),
                        runs: tree,
                        range: 0.into()..chars_len.into(),
                        text_style: None,
                    }
                }
            }

            impl<'a> From<&'a str> for Text {
                fn from(text: &'a str) -> Self {
                    Self::from(String::from(text))
                }
            }

            impl Debug for Text {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.debug_struct("Text")
                        .field("text", &self.text)
                        .field("range", &self.range)
                        .field("text_style", &self.text_style)
                        .finish()
                }
            }

            impl PartialEq for Text {
                fn eq(&self, other: &Self) -> bool {
                    self.text == other.text
                }
            }

            impl Eq for Text {}

            impl<T: RangeBounds<CharOffset>> Index<T> for Text {
                type Output = str;

                fn index(&self, range: T) -> &Self::Output {
                    let start = match range.start_bound() {
                        Bound::Included(start) => {
                            cmp::min(self.range.start + *start, self.range.end)
                        }
                        Bound::Excluded(_) => unimplemented!(),
                        Bound::Unbounded => self.range.start,
                    };
                    let end = match range.end_bound() {
                        Bound::Included(end) => {
                            cmp::min(self.range.start + *end + 1, self.range.end)
                        }
                        Bound::Excluded(end) => cmp::min(self.range.start + *end, self.range.end),
                        Bound::Unbounded => self.range.end,
                    };

                    let byte_start = self.abs_byte_offset_for_offset(start);
                    let byte_end = self.abs_byte_offset_for_offset(end);
                    &self.text[byte_start.as_usize()..byte_end.as_usize()]
                }
            }

            impl Text {
                pub fn range(&self) -> Range<CharOffset> {
                    self.range.clone()
                }

                pub fn as_str(&self) -> &str {
                    &self[..]
                }

                pub fn slice<T: RangeBounds<CharOffset>>(&self, range: T) -> Text {
                    let start = match range.start_bound() {
                        Bound::Included(start) => {
                            cmp::min(self.range.start + *start, self.range.end)
                        }
                        Bound::Excluded(_) => unimplemented!(),
                        Bound::Unbounded => self.range.start,
                    };
                    let end = match range.end_bound() {
                        Bound::Included(end) => {
                            cmp::min(self.range.start + *end + 1, self.range.end)
                        }
                        Bound::Excluded(end) => cmp::min(self.range.start + *end, self.range.end),
                        Bound::Unbounded => self.range.end,
                    };

                    Text {
                        text: self.text.clone(),
                        runs: self.runs.clone(),
                        range: start..end,
                        text_style: self.text_style,
                    }
                }

                pub fn line_len(&self, row: u32) -> u32 {
                    let mut cursor = self.runs.cursor::<CharOffset, Point>();
                    cursor.seek(&self.range.start, SeekBias::Right);
                    let absolute_row = cursor.start().row + row;

                    let mut cursor = self.runs.cursor::<Point, CharOffset>();
                    cursor.seek(&Point::new(absolute_row, 0), SeekBias::Right);
                    let prefix_len = self.range.start.saturating_sub(cursor.start());
                    let line_len = cursor
                        .summary::<CharOffset>(&Point::new(absolute_row + 1, 0), SeekBias::Left);
                    let suffix_len = cursor.start().saturating_sub(&self.range.end);

                    line_len
                        .saturating_sub(&prefix_len)
                        .saturating_sub(&suffix_len)
                        .as_usize() as u32
                }

                pub fn len(&self) -> CharOffset {
                    self.range.end - self.range.start
                }

                pub fn byte_len(&self) -> ByteOffset {
                    self.as_str().len().into()
                }

                pub fn is_empty(&self) -> bool {
                    self.range.is_empty()
                }

                pub fn lines(&self) -> Point {
                    self.abs_point_for_offset(self.range.end)
                        - self.abs_point_for_offset(self.range.start)
                }

                pub fn rightmost_point(&self) -> Point {
                    let lines = self.lines();

                    let mut candidates = ArrayVec::<Point, 3>::new();
                    candidates.push(lines);
                    if lines.row > 0 {
                        candidates.push(Point::new(0, self.line_len(0)));
                        if lines.row > 1 {
                            let mut cursor = self.runs.cursor::<CharOffset, Point>();
                            cursor.seek(&self.range.start, SeekBias::Right);
                            let absolute_start_row = cursor.start().row;

                            let mut cursor = self.runs.cursor::<Point, CharOffset>();
                            cursor.seek(&Point::new(absolute_start_row + 1, 0), SeekBias::Right);
                            let summary = cursor.summary::<TextSummary>(
                                &Point::new(absolute_start_row + lines.row, 0),
                                SeekBias::Left,
                            );

                            candidates.push(Point::new(1, 0) + summary.rightmost_point);
                        }
                    }

                    candidates.into_iter().max_by_key(|p| p.column).unwrap()
                }

                pub fn point_for_offset(&self, offset: CharOffset) -> Point {
                    self.abs_point_for_offset(self.range.start + offset)
                        - self.abs_point_for_offset(self.range.start)
                }

                pub fn offset_for_point(&self, point: Point) -> CharOffset {
                    let mut cursor = self.runs.cursor::<Point, TextSummary>();
                    let abs_point = self.abs_point_for_offset(self.range.start) + point;
                    cursor.seek(&abs_point, SeekBias::Right);
                    let overshoot = abs_point - cursor.start().lines;
                    let abs_offset = cursor.start().chars.as_usize() + overshoot.column as usize;
                    CharOffset::from(abs_offset) - self.range.start
                }

                pub fn byte_offset_for_point(&self, point: Point) -> ByteOffset {
                    // Compute the number of characters the `point` is from the start of the text.
                    let character_offset = self.offset_for_point(point);

                    let num_bytes_to_point =
                        self.abs_byte_offset_for_offset(character_offset + self.range.start);
                    let num_bytes_to_start = self.abs_byte_offset_for_offset(self.range.start);

                    num_bytes_to_point - num_bytes_to_start
                }

                pub fn summary(&self) -> TextSummary {
                    TextSummary {
                        chars: self.range.end - self.range.start,
                        bytes: self.abs_byte_offset_for_offset(self.range.end)
                            - self.abs_byte_offset_for_offset(self.range.start),
                        lines: self.abs_point_for_offset(self.range.end)
                            - self.abs_point_for_offset(self.range.start),
                        first_line_len: self.line_len(0),
                        rightmost_point: self.rightmost_point(),
                    }
                }

                /// Computes the number of equivalent chars from the start of the `Text` given the number of
                /// bytes from the start of the `Text`.
                pub fn char_offset_for_byte_offset(&self, byte_offset: ByteOffset) -> CharOffset {
                    let mut cursor = self.runs.cursor::<ByteOffset, TextSummary>();
                    let abs_byte_offset =
                        self.abs_byte_offset_for_offset(self.range.start) + byte_offset;
                    cursor.seek(&abs_byte_offset, SeekBias::Right);

                    let overshoot = abs_byte_offset - cursor.start().bytes;

                    // Determine the number of characters based on the char size of the fragment.
                    let absolute_chars = cursor.start().chars
                        + ((overshoot).as_usize()
                            / (cursor.item().map_or(1, |run| run.char_size()) as usize));

                    // Convert character offset from the start of the text back to a relative offset back to the
                    // the start of the range.
                    absolute_chars - self.range.start
                }

                fn abs_point_for_offset(&self, offset: CharOffset) -> Point {
                    let mut cursor = self.runs.cursor::<CharOffset, TextSummary>();
                    cursor.seek(&offset, SeekBias::Right);
                    let overshoot = (offset - cursor.start().chars).as_usize() as u32;
                    cursor.start().lines + Point::new(0, overshoot)
                }

                /// Computes the byte offset from the start of the `self.text` given a character offset from
                /// the start of `self.text`.
                fn abs_byte_offset_for_offset(&self, offset: CharOffset) -> ByteOffset {
                    let mut cursor = self.runs.cursor::<CharOffset, TextSummary>();
                    cursor.seek(&offset, SeekBias::Right);
                    let overshoot = (offset - cursor.start().chars).as_usize();
                    cursor.start().bytes
                        + (overshoot * cursor.item().map_or(0, |run| run.char_size()) as usize)
                }
            }
        }

        mod time {
            use std::cmp::{self, Ordering};
            use std::collections::HashMap;
            use std::rc::Rc;

            use serde::{Deserialize, Serialize};
            use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

            const BASE_REPLICA_ID: &str = "0";

            /// A unique ID assigned to every peer in the system.
            #[derive(
                Clone, Hash, Eq, PartialEq, Debug, Ord, PartialOrd, Serialize, Deserialize,
            )]
            pub struct ReplicaId(Rc<String>);

            impl ReplicaId {
                pub fn new(id: impl ToString) -> Self {
                    let id = Self(Rc::new(id.to_string()));
                    debug_assert!(id.0.as_str() != BASE_REPLICA_ID);
                    id
                }

                /// Creates an id that is unique within this process. Buffers are never shared with other
                /// processes, so that is enough to keep replicas apart.
                pub fn random() -> Self {
                    static NEXT_REPLICA_ID: AtomicU64 = AtomicU64::new(1);
                    Self::new(NEXT_REPLICA_ID.fetch_add(1, AtomicOrdering::Relaxed))
                }

                /// A sentinel replica ID for the base text.
                /// The base text insertion is considered to be a replica-less edit.
                pub fn base_replica_id() -> Self {
                    Self(Rc::new(BASE_REPLICA_ID.to_string()))
                }
            }

            impl std::fmt::Display for ReplicaId {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str(self.0.as_str())
                }
            }

            /// A bare [lamport timestamp](https://en.wikipedia.org/wiki/Lamport_timestamp).
            /// Prefer to use the full [`Lamport`] type unless the replica ID is known / fixed.
            #[derive(
                Clone,
                Copy,
                Debug,
                Default,
                Eq,
                Hash,
                PartialEq,
                Ord,
                PartialOrd,
                Serialize,
                Deserialize,
            )]
            pub struct LamportValue(usize);

            impl LamportValue {
                fn next(&self) -> Self {
                    LamportValue(self.0 + 1)
                }
            }

            impl From<usize> for LamportValue {
                fn from(val: usize) -> Self {
                    LamportValue(val)
                }
            }

            /// A [lamport timestamp](https://en.wikipedia.org/wiki/Lamport_timestamp).
            ///
            /// Along with the bare [`LamportValue`], we also store the replica ID to identify
            /// the origin of the event associated to this timestamp. This allows us to achieve
            /// a total ordering of events (see https://en.wikipedia.org/wiki/Lamport_timestamp#Implications).
            #[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
            pub struct Lamport {
                pub replica_id: ReplicaId,
                pub value: LamportValue,
            }

            impl Lamport {
                pub fn new(replica_id: ReplicaId) -> Self {
                    Self {
                        value: LamportValue::default(),
                        replica_id,
                    }
                }

                pub fn tick(&mut self) -> Self {
                    let timestamp = self.clone();
                    self.value.0 += 1;
                    timestamp
                }

                pub fn observe(&mut self, timestamp: &Self) {
                    self.value = cmp::max(self.value, timestamp.value).next();
                }

                pub fn replica_id(&self) -> ReplicaId {
                    self.replica_id.clone()
                }
            }

            impl Ord for Lamport {
                /// When comparing lamport timestamps, we break ties using the replica ID
                /// to get a total ordering.
                fn cmp(&self, other: &Self) -> Ordering {
                    (self.value, &self.replica_id).cmp(&(other.value, &other.replica_id))
                }
            }

            impl PartialOrd for Lamport {
                fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                    Some(self.cmp(other))
                }
            }

            /// A [version vector](https://en.wikipedia.org/wiki/Version_vector) to track
            /// the latest lamport timestamp seen for every peer in the system.
            #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
            pub struct Global(Rc<HashMap<ReplicaId, LamportValue>>);

            impl Default for Global {
                fn default() -> Self {
                    Self::new()
                }
            }

            impl Global {
                pub fn new() -> Self {
                    Global(Rc::new(HashMap::new()))
                }

                pub fn get(&self, replica_id: &ReplicaId) -> LamportValue {
                    self.0.get(replica_id).copied().unwrap_or_default()
                }

                pub fn observe(&mut self, timestamp: &Lamport) {
                    let map = Rc::make_mut(&mut self.0);
                    let value = map.entry(timestamp.replica_id.clone()).or_default();
                    *value = cmp::max(*value, timestamp.value);
                }

                pub fn observe_all(&mut self, other: &Self) {
                    for (replica_id, value) in other.0.as_ref() {
                        self.observe(&Lamport {
                            replica_id: replica_id.clone(),
                            value: *value,
                        });
                    }
                }

                pub fn observed(&self, timestamp: &Lamport) -> bool {
                    self.get(&timestamp.replica_id) >= timestamp.value
                }

                pub fn changed_since(&self, other: &Self) -> bool {
                    self.0
                        .iter()
                        .any(|(replica_id, value)| *value > other.get(replica_id))
                }
            }

            impl PartialOrd for Global {
                /// Returns Some(Ordering) iff the ordering is conclusive.
                /// Otherwise, the version vectors being compared are
                /// concurrent and so the ordering is undefined ([`None`]).
                fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                    let mut global_ordering = Ordering::Equal;

                    for replica_id in self.0.keys().chain(other.0.keys()) {
                        let ordering = self.get(replica_id).cmp(&other.get(replica_id));
                        if ordering != Ordering::Equal {
                            if global_ordering == Ordering::Equal {
                                global_ordering = ordering;
                            } else if ordering != global_ordering {
                                return None;
                            }
                        }
                    }

                    Some(global_ordering)
                }
            }
        }

        mod undo {
            use std::collections::{BTreeSet, HashMap};
            use std::time::Duration;

            use bounded_vec_deque::BoundedVecDeque;
            use instant::Instant;

            use super::time::{Global, Lamport, LamportValue};
            use crate::editor::PlainTextEditorViewAction;
            use crate::editor::model::LocalSelections;

            /// The maximum time we will batch consecutive edits for the same [`Action`].
            /// The "batch" here is not to be confused with the [`Buffer`]'s notion
            /// of a "batch". A single undo / redo batched item might consist
            /// of multiple buffer batched items.
            const UNDO_REDO_BATCH_TIMER: Duration = Duration::from_millis(500);

            /// The maximum size of the undo stack.
            /// TODO: this could be substantially larger now that we don't store snapshots.
            const DEFAULT_UNDO_STACK_CAPACITY: usize = 20;

            /// An entry in the [`LocalUndoStack`].
            #[derive(Clone)]
            struct UndoStackEntry {
                /// The set of operations that would be undone.
                /// These numbers are lamport timestamps corresponding to the edit operations.
                /// We don't need a full [`Lamport`] timestamp because the replica ID would be
                /// redundant (this is only for local edits).
                ops: Vec<LamportValue>,

                /// A snapshot of the selection state after the ops.
                selections: LocalSelections,

                /// The action associated with this entry.
                /// If this is the current item on the stack and the last
                /// change in the current batch was a pure selection change, this will be
                /// [`Action::CursorChanged`].
                action: PlainTextEditorViewAction,
            }

            /// A stack of batched, local operations that can be undone / redone.
            /// The undo stack batches consecutive records within a [`UNDO_REDO_BATCH_TIMER`]
            /// period if:
            /// - the records are associated to the same action
            /// - the associated action is not atomic
            /// - there aren't any undos (we're at the top of the stack)
            #[derive(Clone)]
            pub struct LocalUndoStack {
                /// The stack itself.
                stack: BoundedVecDeque<UndoStackEntry>,

                /// Where we are in the stack currently.
                /// As we undo / redo, the position into the stack
                /// changes but the stack itself stays in tact.
                current_index: usize,

                /// The absolute time when the stack was last modified.
                last_changed_at: Option<Instant>,
            }

            impl LocalUndoStack {
                pub fn new(init_selections: LocalSelections) -> LocalUndoStack {
                    Self::new_with_capacity(init_selections, DEFAULT_UNDO_STACK_CAPACITY)
                }

                fn new_with_capacity(
                    init_selections: LocalSelections,
                    capacity: usize,
                ) -> LocalUndoStack {
                    let init_entry = UndoStackEntry {
                        ops: vec![],
                        selections: init_selections,
                        action: PlainTextEditorViewAction::ReplaceBuffer,
                    };
                    Self {
                        stack: BoundedVecDeque::from_iter([init_entry], capacity),
                        current_index: 0,
                        last_changed_at: None,
                    }
                }

                /// Records a selection change by updating the current item's selection snapshot.
                pub fn record_selection_change(&mut self, selections: LocalSelections) {
                    self.stack[self.current_index].selections = selections;
                    self.stack[self.current_index].action =
                        PlainTextEditorViewAction::CursorChanged;
                }

                /// Records an edit and potentially batches it with other edits if possible.
                pub fn record_edit(
                    &mut self,
                    action: PlainTextEditorViewAction,
                    operations: impl Iterator<Item = LamportValue>,
                    selections: LocalSelections,
                ) {
                    let should_insert_new_entry = self.current_index == 0
                        || self.current_index < self.stack.len() - 1
                        || action.is_atomic()
                        || Some(action) != self.last_action()
                        || self
                            .last_changed_at
                            .is_none_or(|t| t.elapsed() >= UNDO_REDO_BATCH_TIMER);

                    // If the index is positioned somewhere other than the last item,
                    // this means that the user is making an edit after some undo'ing.
                    // Any of the items past the current index are not needed anymore.
                    self.stack.truncate(self.current_index + 1);

                    if should_insert_new_entry {
                        // Since we're using a [`BoundedVecDeque`], pushing to the
                        // back might remove from the front if the stack is at capacity.
                        self.stack.push_back(UndoStackEntry {
                            action,
                            ops: operations.collect(),
                            selections,
                        });
                        self.current_index = self.stack.len() - 1;
                    } else {
                        let back = self.stack.back_mut().unwrap();
                        back.ops.extend(operations);
                        back.selections = selections;
                    }

                    self.last_changed_at = Some(Instant::now());
                }

                /// Processes an undo and returns
                /// 1. the set of operations that need to be undone
                /// 2. the set of selections that need to be restored
                pub fn undo(&mut self) -> Option<(Vec<LamportValue>, LocalSelections)> {
                    if self.current_index == 0 {
                        return None;
                    }

                    self.last_changed_at = None;

                    let ops = self.stack[self.current_index].ops.clone();
                    self.current_index -= 1;
                    let selections = self.stack[self.current_index].selections.clone();

                    Some((ops, selections))
                }

                /// Processes a redo and returns
                /// 1. the set of operations that need to be redone
                /// 2. the set of selections that need to be restored
                pub fn redo(&mut self) -> Option<(Vec<LamportValue>, LocalSelections)> {
                    if self.current_index == self.stack.len() - 1 {
                        return None;
                    }

                    self.last_changed_at = None;

                    self.current_index += 1;
                    let UndoStackEntry {
                        ops, selections, ..
                    } = self.stack[self.current_index].clone();

                    Some((ops, selections))
                }

                /// Resets the undo stack to a single item with the given selections.
                pub fn reset(&mut self, selections: LocalSelections) {
                    let init_entry = UndoStackEntry {
                        ops: vec![],
                        selections,
                        action: PlainTextEditorViewAction::ReplaceBuffer,
                    };
                    self.stack =
                        BoundedVecDeque::from_iter([init_entry], DEFAULT_UNDO_STACK_CAPACITY);
                    self.last_changed_at = None;
                    self.current_index = 0;
                }

                /// Returns the action associated to the last item on the undo stack.
                pub fn last_action(&self) -> Option<PlainTextEditorViewAction> {
                    if self.stack.len() == 1 {
                        return None;
                    };
                    self.stack.back().map(|e| e.action)
                }
            }

            /// A history of local and remote undos.
            #[derive(Clone)]
            pub struct UndoHistory {
                /// A mapping from an edit ID to the lamport timestamps of undo operations for that edit.
                /// The values are bare lamport times because the replica ID of an undo operation will
                /// always be the replica ID for the edit itself (one can only undo their own edits).
                ///
                /// Suppose an undo operation of an edit `e` is defined as `undo(e)`.
                /// Then, a redo of an edit operation `e` can be thought of as `undo(undo(e))`.
                /// Using that line of logic, an edit is considered undone iff the number of undos are odd.
                ///
                /// We use a [`BTreeSet`] to quickly query the max undo ts for a given edit
                /// and to efficiently compute intersections with other sets (e.g. deletions).
                map: HashMap<Lamport, BTreeSet<LamportValue>>,
            }

            impl UndoHistory {
                pub fn new() -> Self {
                    UndoHistory {
                        map: HashMap::new(),
                    }
                }

                /// Returns the largest known undo timestamp for `edit`.
                fn max_undo_ts(&self, edit_id: &Lamport) -> LamportValue {
                    self.map
                        .get(edit_id)
                        .and_then(|v| v.last().copied())
                        .unwrap_or_default()
                }

                /// Registers an undo for `edit`
                pub fn undo(&mut self, edit_id: Lamport, undo_ts: LamportValue) {
                    // The undo timestamp should be larger than all undo timestamps
                    // so far for this edit and must be after the edit itself.
                    debug_assert!(undo_ts > self.max_undo_ts(&edit_id) && undo_ts > edit_id.value);
                    self.map.entry(edit_id).or_default().insert(undo_ts);
                }

                /// Returns true iff `edit` is considered undone.
                /// An edit is considered undone iff it has an odd number of undos.
                /// In other words, there exists some undo that wasn't redone.
                pub fn is_edit_undone(&self, edit_id: &Lamport) -> bool {
                    self.map.get(edit_id).map_or(0, |v| v.len()) % 2 == 1
                }

                /// Returns true iff `edit` was undone when the version vector was `version`.
                pub fn was_edit_undone(&self, edit_id: &Lamport, version: &Global) -> bool {
                    let num_undos = self
                        .map
                        .get(edit_id)
                        .iter()
                        .flat_map(|v| v.iter())
                        .take_while(|undo| {
                            version.observed(&Lamport {
                                replica_id: edit_id.replica_id(),
                                value: **undo,
                            })
                        })
                        .count();
                    num_undos % 2 == 1
                }
            }

            impl PlainTextEditorViewAction {
                /// Whether the action is atomic. An atomic action is one where a single event
                /// is independently undoable. This differs from non-atomic actions (such as `INSERT_TEXT`)
                /// where multiple events are coalesced and undone together if they occur within [`UNDO_REDO_BATCH_TIMER`] of each other.
                pub fn is_atomic(&self) -> bool {
                    matches!(
                        self,
                        PlainTextEditorViewAction::Yank
                            | PlainTextEditorViewAction::NewLine
                            | PlainTextEditorViewAction::DeleteWordLeft
                            | PlainTextEditorViewAction::DeleteWordRight
                            | PlainTextEditorViewAction::ReplaceBuffer
                            | PlainTextEditorViewAction::Paste
                            | PlainTextEditorViewAction::ClearLines
                            | PlainTextEditorViewAction::ClearAndCopyLines
                            | PlainTextEditorViewAction::DeleteAll
                            | PlainTextEditorViewAction::CutAll
                            | PlainTextEditorViewAction::InsertSelectedText
                            | PlainTextEditorViewAction::CutWordRight
                            | PlainTextEditorViewAction::SystemInsert
                    )
                }
            }
        }
    }

    mod display_map {

        use std::cmp;
        use std::ops::Range;

        use anyhow::{Context, Result};
        pub use fold_map::BufferRows;
        use fold_map::FoldMap;
        use string_offset::CharOffset;
        use warpui::text::point::Point;
        use warpui::{AppContext, Entity, ModelContext, ModelHandle};

        use super::buffer::{self, Anchor, Buffer, Edit, StylizedChar, ToCharOffset, ToPoint};
        use crate::editor::soft_wrap::{
            self, DisplayPointAndClampDirection, SoftWrapPoint, SoftWrapState,
        };

        #[derive(Copy, Clone)]
        pub enum Bias {
            Left,
            Right,
        }

        pub struct DisplayMap {
            buffer: ModelHandle<Buffer>,
            fold_map: FoldMap,
            soft_wrap_state: SoftWrapState,
            tab_size: usize,
        }

        pub struct MovementResult {
            pub point_and_clamp_direction: DisplayPointAndClampDirection,
            /// True if the resulting point is the same row as the original row.
            pub is_same_row: bool,
            /// The desired column based on the original point. This could be higher than
            /// the returned column in point because the current row may not have had as
            /// many columns as the previous row.
            pub goal_column: u32,
        }

        pub enum Event {
            Folded,
            Unfolded,
        }

        impl Entity for DisplayMap {
            type Event = Event;
        }

        impl DisplayMap {
            fn new_internal(
                buffer: ModelHandle<Buffer>,
                subscribe_to_buffer: bool,
                tab_size: usize,
                ctx: &mut ModelContext<Self>,
            ) -> Self {
                if subscribe_to_buffer {
                    ctx.subscribe_to_model(&buffer, Self::handle_buffer_event);
                }

                DisplayMap {
                    buffer: buffer.clone(),
                    fold_map: FoldMap::new(buffer, ctx),
                    soft_wrap_state: Default::default(),
                    tab_size,
                }
            }

            pub fn new(
                buffer: ModelHandle<Buffer>,
                tab_size: usize,
                ctx: &mut ModelContext<Self>,
            ) -> Self {
                Self::new_internal(buffer, true, tab_size, ctx)
            }

            pub fn recreate(&mut self, tab_size: usize, ctx: &mut ModelContext<Self>) {
                *self = Self::new_internal(self.buffer.clone(), false, tab_size, ctx);
            }

            pub fn buffer<'a>(&self, app: &'a AppContext) -> &'a Buffer {
                self.buffer.as_ref(app)
            }

            pub fn soft_wrap_state(&self) -> SoftWrapState {
                self.soft_wrap_state.clone()
            }

            pub fn fold<T: ToCharOffset>(
                &mut self,
                ranges: impl IntoIterator<Item = Range<T>>,
                ctx: &mut ModelContext<Self>,
            ) -> Result<()> {
                self.fold_map.fold(ranges, ctx)?;
                ctx.emit(Event::Folded);
                Ok(())
            }

            pub fn unfold<T: ToCharOffset>(
                &mut self,
                ranges: impl IntoIterator<Item = Range<T>>,
                ctx: &mut ModelContext<Self>,
            ) -> Result<()> {
                self.fold_map.unfold(ranges, ctx)?;
                ctx.emit(Event::Unfolded);
                Ok(())
            }

            /// TODO(zheng) Consolidate logic with `down`.
            pub fn up(
                &self,
                point: DisplayPoint,
                goal_column: Option<u32>,
                clamp_direction: soft_wrap::ClampDirection,
            ) -> Result<MovementResult> {
                self.soft_wrap_state.read(|frame_layouts| {
            let frame_layouts = frame_layouts.map_err(|err| anyhow::anyhow!("Could not read Frame Layouts {:?}", err))?;
            let point = frame_layouts.to_soft_wrap_point(point, clamp_direction).context("Point is out of bounds of laid out text")?;

            let line = frame_layouts
                .get_line(point.row() as usize)
                .context("Should have current line as it should not be possible for it to be beyond bounds, but was not able to retrieve it")?;
            // Note that the index here will correspond to that of the original text so if
            // this line is soft wrapped, it will be higher than 0.
            let line_first_index = line.first_glyph().map_or(0, |glyph| glyph.index);

            let relative_index = point.column() - line_first_index as u32;

            // Note that, as also mentioned in the rustdoc of this function,
            // the goal column is a relative index to the first point of the row
            // so it starts at 0 and is at most the length of a row (but it could
            // be higher than the current row as it is preserved across rows).
            let goal_column = match goal_column {
                Some(goal_column) => cmp::max(goal_column, relative_index),
                None => relative_index,
            };
            let (point, is_same_row) = if point.row() > 0 {
                let prev_line = frame_layouts
                    .get_line(point.row() as usize - 1)
                    .context("Should have next line based on max point bounds check, but was not able to retrieve it")?;
                let prev_line_first_column = prev_line.first_glyph().map_or(0, |glyph| glyph.index as u32);
                // Note that goal column is a relative value, rather than based
                // on the original string.
                let new_column = prev_line_first_column + goal_column;

                let last_index_in_line = prev_line.last_glyph().map(|glyph| glyph.index as u32 + 1)
                    .unwrap_or(0);

                (SoftWrapPoint::new(
                    point.row() - 1,
                    cmp::min(new_column, last_index_in_line),
                ), false)
            } else {
                (SoftWrapPoint::new(0, 0), true)
            };
            Ok(MovementResult {
                point_and_clamp_direction: frame_layouts.to_display_point(point),
                goal_column,
                is_same_row,
            })
        })
            }

            /// Given a `DisplayPoint` and the goal column (i.e. the rightmost column
            /// that we _want_ to be at which could be higher than the current point's
            /// column) returns the point and new goal column we would be at if we were
            /// to navigate one row down from the point with soft wrap in consideration.
            ///
            /// Note that while `goal_column` is the column number of the row, the column
            /// numbers in `SoftWrapPoint`, are relative to the original string.
            /// TODO(zheng) Consolidate logic with `up`.
            pub fn down(
                &self,
                point: DisplayPoint,
                goal_column: Option<u32>,
                clamp_direction: soft_wrap::ClampDirection,
            ) -> Result<MovementResult> {
                self.soft_wrap_state.read(|frame_layouts| {
            let frame_layouts = frame_layouts.map_err(|err| anyhow::anyhow!("Could not read Frame Layouts {:?}", err))?;
            let point = frame_layouts.to_soft_wrap_point(point, clamp_direction).ok_or_else(|| anyhow::anyhow!("Point is out of bounds of laid out text"))?;

            let line = frame_layouts
                .get_line(point.row() as usize)
                .context("Should have current line as it should not be possible for it to be beyond bounds, but was not able to retrieve it")?;
            // Note that the index here will correspond to that of the original text so if
            // this line is soft wrapped, it will be higher than 0.
            let line_first_index = line.first_glyph().map_or(0, |glyph| glyph.index);

            let relative_index = point.column() - line_first_index as u32;

            // Note that, as also mentioned in the rustdoc of this function,
            // the goal column is a relative index to the first point of the row
            // so it starts at 0 and is at most the length of a row (but it could
            // be higher than the current row as it is preserved across rows).
            let goal_column = match goal_column {
                Some(goal_column) => cmp::max(goal_column, relative_index),
                None => relative_index,
            };
            let max_row = frame_layouts.num_lines() - 1;
            let max_col = frame_layouts
                .get_line(max_row)
                .and_then(|line| line.last_glyph().map(|glyph| glyph.index + 1))
                .unwrap_or(0);
            let max_point = SoftWrapPoint::new(max_row as u32, max_col as u32);
            let (point, is_same_row) = if point.row() < max_point.row() {
                let next_line = frame_layouts
                    .get_line(point.row() as usize + 1)
                    .context("Should have next line based on max point bounds check, but was not able to retrieve it")?;
                let next_line_first_column = next_line.first_glyph().map_or(0, |glyph| glyph.index as u32);
                // Note that goal column is a relative value, rather than based
                // on the original string.
                let new_column = next_line_first_column + goal_column;

                let last_index_in_line = next_line.last_glyph().map(|glyph| glyph.index as u32 + 1)
                    .unwrap_or(0);

                (SoftWrapPoint::new(
                    point.row() + 1,
                    cmp::min(new_column, last_index_in_line),
                ), false)
            } else {
                (max_point, true)
            };
            Ok(MovementResult {
                point_and_clamp_direction: frame_layouts.to_display_point(point),
                goal_column,
                is_same_row,
            })
        })
            }

            /// Returns the `[start, end)` display columns of the visual (soft-wrapped)
            /// row that `point` lies on, or `None` if the soft-wrap layout is
            /// unavailable (e.g. before the first frame of the editor has been laid
            /// out). Callers should fall back to logical-line behavior in that case.
            pub fn soft_wrapped_row_bounds(
                &self,
                point: DisplayPoint,
                clamp_direction: soft_wrap::ClampDirection,
            ) -> Option<Range<u32>> {
                self.soft_wrap_state.read(|frame_layouts| {
                    frame_layouts
                        .ok()?
                        .soft_wrapped_row_bounds(point, clamp_direction)
                })
            }

            pub fn to_soft_wrap_point(
                &self,
                point: DisplayPoint,
                clamp_direction: soft_wrap::ClampDirection,
            ) -> Option<SoftWrapPoint> {
                self.soft_wrap_state
                    .read(|frame_layouts| match frame_layouts {
                        Ok(frame_layouts) => {
                            frame_layouts.to_soft_wrap_point(point, clamp_direction)
                        }
                        Err(err) => {
                            log::warn!("Error attempting to get soft wrap point {err:?}");
                            None
                        }
                    })
            }

            pub fn is_line_folded(&self, display_row: u32) -> bool {
                self.fold_map.is_line_folded(display_row)
            }

            pub fn tab_size(&self) -> usize {
                self.tab_size
            }

            pub fn line(&self, display_row: u32, app: &AppContext) -> Result<String> {
                let chars = self.chars_at(DisplayPoint::new(display_row, 0), app)?;
                Ok(chars.take_while(|c| *c != '\n').collect())
            }

            pub fn chars_with_styles_at<'a>(
                &'a self,
                point: DisplayPoint,
                app: &'a AppContext,
            ) -> Result<impl 'a + Iterator<Item = StylizedChar>> {
                let column = point.column() as usize;
                let (point, to_next_stop) = point.collapse_tabs(self, Bias::Left, app)?;
                let mut fold_chars = self.fold_map.chars_with_style_at(point, app)?;
                if to_next_stop > 0 {
                    fold_chars.next();
                }

                Ok(CharsWithStyles {
                    fold_chars,
                    column,
                    to_next_stop,
                    tab_size: self.tab_size,
                })
            }

            pub fn chars_at<'a>(
                &'a self,
                point: DisplayPoint,
                app: &'a AppContext,
            ) -> Result<impl 'a + Iterator<Item = char>> {
                Ok(self.chars_with_styles_at(point, app)?.map(Into::into))
            }

            pub fn buffer_rows(&self, start_row: u32) -> Result<BufferRows<'_>> {
                self.fold_map.buffer_rows(start_row)
            }

            pub fn line_len(&self, row: u32, ctx: &AppContext) -> Result<u32> {
                DisplayPoint::new(row, self.fold_map.line_len(row, ctx)?)
                    .expand_tabs(self, ctx)
                    .map(|point| point.column())
            }

            pub fn max_point(&self, app: &AppContext) -> DisplayPoint {
                self.fold_map.max_point().expand_tabs(self, app).unwrap()
            }

            pub fn rightmost_point(&self) -> DisplayPoint {
                self.fold_map.rightmost_point()
            }

            pub fn anchor_before(
                &self,
                point: DisplayPoint,
                bias: Bias,
                app: &AppContext,
            ) -> Result<Anchor> {
                self.buffer
                    .as_ref(app)
                    .anchor_before(point.to_buffer_point(self, bias, app)?)
            }

            #[allow(dead_code)]
            pub fn anchor_after(
                &self,
                point: DisplayPoint,
                bias: Bias,
                app: &AppContext,
            ) -> Result<Anchor> {
                self.buffer
                    .as_ref(app)
                    .anchor_after(point.to_buffer_point(self, bias, app)?)
            }

            pub fn apply_edits(&mut self, edits: &[Edit], ctx: &AppContext) -> Result<()> {
                self.fold_map.apply_edits(edits, ctx)
            }

            fn handle_buffer_event(
                &mut self,
                _: ModelHandle<Buffer>,
                event: &buffer::Event,
                ctx: &mut ModelContext<Self>,
            ) {
                match event {
                    buffer::Event::Edited { edits, .. } => self.apply_edits(edits, ctx).unwrap(),
                    buffer::Event::StylesUpdated | buffer::Event::SelectionsChanged => {}
                }
            }
        }

        #[derive(Copy, Clone, Debug, Default, Eq, Ord, PartialOrd, PartialEq)]
        pub struct DisplayPoint(Point);

        impl DisplayPoint {
            pub fn new(row: u32, column: u32) -> Self {
                Self(Point::new(row, column))
            }

            #[allow(dead_code)]
            pub fn zero() -> Self {
                Self::new(0, 0)
            }

            pub fn row(self) -> u32 {
                self.0.row
            }

            pub fn column(self) -> u32 {
                self.0.column
            }

            pub fn row_mut(&mut self) -> &mut u32 {
                &mut self.0.row
            }

            pub fn column_mut(&mut self) -> &mut u32 {
                &mut self.0.column
            }

            pub fn to_buffer_point(
                self,
                map: &DisplayMap,
                bias: Bias,
                app: &AppContext,
            ) -> Result<Point> {
                Ok(map
                    .fold_map
                    .to_buffer_point(self.collapse_tabs(map, bias, app)?.0))
            }

            pub fn to_char_offset(
                self,
                map: &DisplayMap,
                bias: Bias,
                buffer: &Buffer,
                app: &AppContext,
            ) -> Result<CharOffset> {
                let point = self.to_buffer_point(map, bias, app)?;
                point.to_char_offset(buffer)
            }

            fn expand_tabs(mut self, map: &DisplayMap, app: &AppContext) -> Result<Self> {
                let chars = map
                    .fold_map
                    .chars_at(DisplayPoint(Point::new(self.row(), 0)), app)?;
                let expanded = expand_tabs(chars, self.column() as usize, map.tab_size);
                *self.column_mut() = expanded as u32;

                Ok(self)
            }

            fn collapse_tabs(
                mut self,
                map: &DisplayMap,
                bias: Bias,
                app: &AppContext,
            ) -> Result<(Self, usize)> {
                let chars = map
                    .fold_map
                    .chars_at(DisplayPoint(Point::new(self.0.row, 0)), app)?;
                let expanded = self.column() as usize;
                let (collapsed, to_next_stop) = collapse_tabs(chars, expanded, bias, map.tab_size);
                *self.column_mut() = collapsed as u32;

                Ok((self, to_next_stop))
            }
        }

        /// Trait for types that can map onto a display point, accounting for soft-wrapping
        /// and text folding.
        pub trait ToDisplayPoint {
            fn to_display_point(self, map: &DisplayMap, app: &AppContext) -> Result<DisplayPoint>;
        }

        impl ToDisplayPoint for Point {
            fn to_display_point(self, map: &DisplayMap, app: &AppContext) -> Result<DisplayPoint> {
                let mut display_point = map.fold_map.to_display_point(self);
                let chars = map
                    .fold_map
                    .chars_at(DisplayPoint::new(display_point.row(), 0), app)?;
                *display_point.column_mut() =
                    expand_tabs(chars, display_point.column() as usize, map.tab_size) as u32;
                Ok(display_point)
            }
        }

        impl ToDisplayPoint for &Anchor {
            fn to_display_point(self, map: &DisplayMap, app: &AppContext) -> Result<DisplayPoint> {
                self.to_point(map.buffer.as_ref(app))?
                    .to_display_point(map, app)
            }
        }

        pub struct CharsWithStyles<'a> {
            fold_chars: fold_map::CharsWithStyle<'a>,
            column: usize,
            to_next_stop: usize,
            tab_size: usize,
        }

        impl Iterator for CharsWithStyles<'_> {
            type Item = StylizedChar;

            fn next(&mut self) -> Option<Self::Item> {
                if self.to_next_stop > 0 {
                    self.to_next_stop -= 1;
                    self.column += 1;
                    Some(StylizedChar::new(' ', Default::default()))
                } else {
                    self.fold_chars.next().map(|c| match c.char() {
                        '\t' => {
                            self.to_next_stop = self.tab_size - self.column % self.tab_size - 1;
                            self.column += 1;
                            StylizedChar::new(' ', c.style())
                        }
                        '\n' => {
                            self.column = 0;
                            c
                        }
                        _ => {
                            self.column += 1;
                            c
                        }
                    })
                }
            }
        }

        pub fn expand_tabs(
            chars: impl Iterator<Item = char>,
            column: usize,
            tab_size: usize,
        ) -> usize {
            let mut expanded = 0;
            for c in chars.take(column) {
                if c == '\t' {
                    expanded += tab_size - expanded % tab_size;
                } else {
                    expanded += 1;
                }
            }
            expanded
        }

        pub fn collapse_tabs(
            chars: impl Iterator<Item = char>,
            column: usize,
            bias: Bias,
            tab_size: usize,
        ) -> (usize, usize) {
            let mut expanded = 0;
            let mut collapsed = 0;
            for c in chars {
                if expanded == column {
                    break;
                }

                if c == '\t' {
                    expanded += tab_size - (expanded % tab_size);
                    if expanded > column {
                        return match bias {
                            Bias::Left => (collapsed, expanded - column),
                            Bias::Right => (collapsed + 1, 0),
                        };
                    }
                } else {
                    expanded += 1;
                }
                collapsed += 1;
            }
            (collapsed, 0)
        }

        mod fold_map {
            use std::cmp::{self, Ordering};
            use std::iter::Take;
            use std::ops::Range;

            use anyhow::{Result, anyhow};
            use string_offset::CharOffset;
            use sum_tree::{self, Cursor, Dimension, SeekBias, SumTree};
            use warpui::text_layout::TextStyle;
            use warpui::{AppContext, ModelHandle};

            use super::super::buffer::{AnchorRangeExt, TextSummary};
            use super::buffer::StylizedChar;
            use super::{Anchor, Buffer, DisplayPoint, Edit, Point, ToCharOffset, buffer};
            use crate::editor::SliceExt as _;

            pub struct FoldMap {
                buffer: ModelHandle<Buffer>,
                transforms: SumTree<Transform>,
                folds: Vec<Range<Anchor>>,
            }

            impl FoldMap {
                pub fn new(buffer: ModelHandle<Buffer>, app: &AppContext) -> Self {
                    let text_summary = buffer.as_ref(app).text_summary();
                    Self {
                        buffer,
                        folds: Vec::new(),
                        transforms: SumTree::from_item(Transform {
                            summary: TransformSummary {
                                buffer: text_summary.clone(),
                                display: text_summary,
                            },
                            display_text: None,
                        }),
                    }
                }

                pub fn buffer_rows(&self, start_row: u32) -> Result<BufferRows<'_>> {
                    if start_row > self.transforms.summary().display.lines.row {
                        return Err(anyhow!("invalid display row {}", start_row));
                    }

                    let display_point = Point::new(start_row, 0);
                    let mut cursor = self.transforms.cursor();
                    cursor.seek(&DisplayPoint(display_point), SeekBias::Left);

                    Ok(BufferRows {
                        cursor,
                        display_point,
                    })
                }

                pub fn len(&self) -> CharOffset {
                    self.transforms.summary().display.chars
                }

                pub fn line_len(&self, row: u32, ctx: &AppContext) -> Result<u32> {
                    let line_start = self.to_display_offset(DisplayPoint::new(row, 0), ctx)?.0;
                    let line_end = if row >= self.max_point().row() {
                        self.len().as_usize()
                    } else {
                        self.to_display_offset(DisplayPoint::new(row + 1, 0), ctx)?
                            .0
                            - 1
                    };

                    Ok((line_end - line_start) as u32)
                }

                pub fn chars_with_style_at<'a>(
                    &'a self,
                    point: DisplayPoint,
                    app: &'a AppContext,
                ) -> Result<CharsWithStyle<'a>> {
                    let offset = self.to_display_offset(point, app)?;
                    let mut cursor = self.transforms.cursor();
                    cursor.seek(&offset, SeekBias::Right);
                    let buffer = self.buffer.as_ref(app);
                    Ok(CharsWithStyle {
                        cursor,
                        offset: CharOffset::from(offset.0),
                        buffer,
                        buffer_chars: None,
                    })
                }

                pub fn chars_at<'a>(
                    &'a self,
                    point: DisplayPoint,
                    app: &'a AppContext,
                ) -> Result<Chars<'a>> {
                    Ok(Chars(self.chars_with_style_at(point, app)?))
                }

                pub fn max_point(&self) -> DisplayPoint {
                    DisplayPoint(self.transforms.summary().display.lines)
                }

                pub fn rightmost_point(&self) -> DisplayPoint {
                    DisplayPoint(self.transforms.summary().display.rightmost_point)
                }

                pub fn fold<T: ToCharOffset>(
                    &mut self,
                    ranges: impl IntoIterator<Item = Range<T>>,
                    app: &AppContext,
                ) -> Result<()> {
                    let mut edits = Vec::new();
                    let buffer = self.buffer.as_ref(app);
                    for range in ranges.into_iter() {
                        let start = range.start.to_char_offset(buffer)?;
                        let end = range.end.to_char_offset(buffer)?;
                        edits.push(Edit {
                            old_range: start..end,
                            new_range: start..end,
                        });

                        let fold = buffer.anchor_after(start)?..buffer.anchor_before(end)?;
                        let ix = self
                            .folds
                            .find_insertion_index(|probe| probe.cmp(&fold, buffer))?;
                        self.folds.insert(ix, fold);
                    }
                    edits.sort_unstable_by(|a, b| {
                        a.old_range
                            .start
                            .cmp(&b.old_range.start)
                            .then_with(|| b.old_range.end.cmp(&a.old_range.end))
                    });

                    self.apply_edits(&edits, app)?;
                    Ok(())
                }

                pub fn unfold<T: ToCharOffset>(
                    &mut self,
                    ranges: impl IntoIterator<Item = Range<T>>,
                    app: &AppContext,
                ) -> Result<()> {
                    let buffer = self.buffer.as_ref(app);

                    let mut edits = Vec::new();
                    for range in ranges.into_iter() {
                        let start = buffer.anchor_before(range.start.to_char_offset(buffer)?)?;
                        let end = buffer.anchor_after(range.end.to_char_offset(buffer)?)?;

                        // Remove intersecting folds and add their ranges to edits that are passed to apply_edits
                        self.folds.retain(|fold| {
                            if fold.start.cmp(&end, buffer).unwrap() > Ordering::Equal
                                || fold.end.cmp(&start, buffer).unwrap() < Ordering::Equal
                            {
                                true
                            } else {
                                let start = fold.start.to_char_offset(buffer);
                                let end = fold.end.to_char_offset(buffer);
                                if let Ok((start, end)) = start.and_then(|start| Ok((start, end?)))
                                {
                                    let offset_range = start..end;
                                    edits.push(Edit {
                                        old_range: offset_range.clone(),
                                        new_range: offset_range,
                                    });
                                }

                                false
                            }
                        });
                    }

                    self.apply_edits(&edits, app)?;
                    Ok(())
                }

                pub fn is_line_folded(&self, display_row: u32) -> bool {
                    let mut cursor = self.transforms.cursor::<DisplayPoint, DisplayPoint>();
                    cursor.seek(&DisplayPoint::new(display_row, 0), SeekBias::Right);
                    while let Some(transform) = cursor.item() {
                        if transform.display_text.is_some() {
                            return true;
                        }
                        if cursor.end().row() == display_row {
                            cursor.next()
                        } else {
                            break;
                        }
                    }
                    false
                }

                pub fn to_display_offset(
                    &self,
                    point: DisplayPoint,
                    app: &AppContext,
                ) -> Result<DisplayOffset> {
                    let mut cursor = self.transforms.cursor::<DisplayPoint, TransformSummary>();
                    cursor.seek(&point, SeekBias::Right);
                    let overshoot = point.0 - cursor.start().display.lines;
                    let mut offset = cursor.start().display.chars;
                    if !overshoot.is_zero() {
                        let transform = cursor
                            .item()
                            .ok_or_else(|| anyhow!("display point {:?} is out of range", point))?;
                        assert!(transform.display_text.is_none());
                        let end_buffer_offset = (cursor.start().buffer.lines + overshoot)
                            .to_char_offset(self.buffer.as_ref(app))?;
                        offset += end_buffer_offset - cursor.start().buffer.chars;
                    }
                    Ok(offset.into())
                }

                pub fn to_buffer_point(&self, display_point: DisplayPoint) -> Point {
                    let mut cursor = self.transforms.cursor::<DisplayPoint, TransformSummary>();
                    cursor.seek(&display_point, SeekBias::Right);
                    let overshoot = display_point.0 - cursor.start().display.lines;
                    cursor.start().buffer.lines + overshoot
                }

                pub fn to_display_point(&self, point: Point) -> DisplayPoint {
                    let mut cursor = self.transforms.cursor::<Point, TransformSummary>();
                    cursor.seek(&point, SeekBias::Right);
                    let overshoot = point - cursor.start().buffer.lines;
                    DisplayPoint(cmp::min(
                        cursor.start().display.lines + overshoot,
                        cursor.end().display.lines,
                    ))
                }

                pub fn apply_edits(&mut self, edits: &[Edit], app: &AppContext) -> Result<()> {
                    let buffer = self.buffer.as_ref(app);
                    let mut edits = edits.iter().cloned().peekable();

                    let mut new_transforms = SumTree::new();
                    let mut cursor = self.transforms.cursor::<CharOffset, CharOffset>();
                    cursor.seek(&0.into(), SeekBias::Right);

                    while let Some(mut edit) = edits.next() {
                        new_transforms
                            .push_tree(cursor.slice(&edit.old_range.start, SeekBias::Left));
                        edit.new_range.start -= edit.old_range.start - *cursor.start();
                        edit.old_range.start = *cursor.start();

                        cursor.seek(&edit.old_range.end, SeekBias::Right);
                        cursor.next();

                        let mut delta = edit.delta();
                        loop {
                            edit.old_range.end = *cursor.start();

                            if let Some(next_edit) = edits.peek() {
                                if next_edit.old_range.start > edit.old_range.end {
                                    break;
                                }

                                let next_edit = edits.next().unwrap();
                                delta += next_edit.delta();

                                if next_edit.old_range.end > edit.old_range.end {
                                    edit.old_range.end = next_edit.old_range.end;
                                    cursor.seek(&edit.old_range.end, SeekBias::Right);
                                    cursor.next();
                                }
                            } else {
                                break;
                            }
                        }

                        edit.new_range.end = CharOffset::from(
                            ((edit.new_range.start + edit.old_extent()).as_usize() as isize + delta)
                                as usize,
                        );

                        let anchor = buffer.anchor_before(edit.new_range.start)?;
                        let folds_start = self
                            .folds
                            .find_insertion_index(|probe| probe.start.cmp(&anchor, buffer))?;
                        let mut folds = self.folds[folds_start..]
                            .iter()
                            .filter_map(|fold| {
                                Some(
                                    fold.start.to_char_offset(buffer).ok()?
                                        ..fold.end.to_char_offset(buffer).ok()?,
                                )
                            })
                            .peekable();

                        while folds
                            .peek()
                            .is_some_and(|fold| fold.start < edit.new_range.end)
                        {
                            let mut fold = folds.next().unwrap();
                            let sum = new_transforms.summary();

                            assert!(fold.start >= sum.buffer.chars);

                            while folds
                                .peek()
                                .is_some_and(|next_fold| next_fold.start <= fold.end)
                            {
                                let next_fold = folds.next().unwrap();
                                if next_fold.end > fold.end {
                                    fold.end = next_fold.end;
                                }
                            }

                            if fold.start > sum.buffer.chars {
                                let text_summary =
                                    buffer.text_summary_for_range(sum.buffer.chars..fold.start);
                                new_transforms.push(Transform {
                                    summary: TransformSummary {
                                        display: text_summary.clone(),
                                        buffer: text_summary,
                                    },
                                    display_text: None,
                                });
                            }

                            if fold.end > fold.start {
                                new_transforms.push(Transform {
                                    summary: TransformSummary {
                                        display: TextSummary {
                                            chars: 1.into(),
                                            bytes: '…'.len_utf8().into(),
                                            lines: Point::new(0, 1),
                                            first_line_len: 1,
                                            rightmost_point: Point::new(0, 1),
                                        },
                                        buffer: buffer.text_summary_for_range(fold.start..fold.end),
                                    },
                                    display_text: Some('…'),
                                });
                            }
                        }

                        let sum = new_transforms.summary();
                        if sum.buffer.chars < edit.new_range.end {
                            let text_summary =
                                buffer.text_summary_for_range(sum.buffer.chars..edit.new_range.end);
                            new_transforms.push(Transform {
                                summary: TransformSummary {
                                    display: text_summary.clone(),
                                    buffer: text_summary,
                                },
                                display_text: None,
                            });
                        }
                    }

                    new_transforms.push_tree(cursor.suffix());

                    drop(cursor);
                    self.transforms = new_transforms;

                    Ok(())
                }
            }

            #[derive(Clone, Debug, Default, Eq, PartialEq)]
            struct Transform {
                summary: TransformSummary,
                display_text: Option<char>,
            }

            #[derive(Clone, Debug, Default, Eq, PartialEq)]
            struct TransformSummary {
                display: TextSummary,
                buffer: TextSummary,
            }

            impl sum_tree::Item for Transform {
                type Summary = TransformSummary;

                fn summary(&self) -> Self::Summary {
                    self.summary.clone()
                }
            }

            impl<'a> std::ops::AddAssign<&'a Self> for TransformSummary {
                fn add_assign(&mut self, other: &'a Self) {
                    self.buffer += &other.buffer;
                    self.display += &other.display;
                }
            }

            impl<'a> Dimension<'a, TransformSummary> for TransformSummary {
                fn add_summary(&mut self, summary: &'a TransformSummary) {
                    *self += summary;
                }
            }

            pub struct BufferRows<'a> {
                cursor: Cursor<'a, Transform, DisplayPoint, TransformSummary>,
                display_point: Point,
            }

            impl Iterator for BufferRows<'_> {
                type Item = u32;

                fn next(&mut self) -> Option<Self::Item> {
                    while self.display_point > self.cursor.end().display.lines {
                        self.cursor.next();
                        if self.cursor.item().is_none() {
                            // TODO: Return a bool from next?
                            break;
                        }
                    }

                    if self.cursor.item().is_some() {
                        let overshoot = self.display_point - self.cursor.start().display.lines;
                        let buffer_point = self.cursor.start().buffer.lines + overshoot;
                        self.display_point.row += 1;
                        Some(buffer_point.row)
                    } else {
                        None
                    }
                }
            }

            pub struct Chars<'a>(CharsWithStyle<'a>);

            impl Iterator for Chars<'_> {
                type Item = char;

                fn next(&mut self) -> Option<Self::Item> {
                    self.0.next().map(Into::into)
                }
            }

            pub struct CharsWithStyle<'a> {
                cursor: Cursor<'a, Transform, DisplayOffset, TransformSummary>,
                offset: CharOffset,
                buffer: &'a Buffer,
                buffer_chars: Option<Take<buffer::CharsWithStyle<'a>>>,
            }

            impl Iterator for CharsWithStyle<'_> {
                type Item = StylizedChar;

                fn next(&mut self) -> Option<Self::Item> {
                    match self.buffer_chars.as_mut().map(Iterator::next) {
                        // buffer_chars is set and has a value, return it
                        Some(Some(c)) => {
                            self.offset += 1;
                            return Some(c);
                        }
                        // buffer_chars is set, but exhausted, so the current cursor item is complete
                        Some(None) => {
                            self.buffer_chars = None;
                            self.cursor.next();
                        }
                        // buffer_chars is not set, if we've returned all of the current cursor item's
                        // characters, then we should move to the next value
                        None => {
                            if self.offset == self.cursor.end().display.chars {
                                self.cursor.next();
                            }
                        }
                    }

                    self.cursor.item().and_then(|transform| {
                        if let Some(c) = transform.display_text {
                            self.offset += 1;
                            Some(StylizedChar::new(c, TextStyle::default()))
                        } else {
                            let overshoot = self.offset - self.cursor.start().display.chars;
                            let buffer_start = self.cursor.start().buffer.chars + overshoot;
                            let char_count = self.cursor.end().buffer.chars - buffer_start;
                            self.buffer_chars = Some(
                                self.buffer
                                    .stylized_chars_at(buffer_start)
                                    .unwrap()
                                    .take(char_count.as_usize()),
                            );
                            self.next()
                        }
                    })
                }
            }

            impl<'a> Dimension<'a, TransformSummary> for DisplayPoint {
                fn add_summary(&mut self, summary: &'a TransformSummary) {
                    self.0 += summary.display.lines;
                }
            }

            #[derive(Copy, Clone, Debug, Default, Eq, Ord, PartialOrd, PartialEq)]
            /// The number of _visible_ characters offset from the start of the buffer.
            pub struct DisplayOffset(usize);

            impl From<usize> for DisplayOffset {
                fn from(usize: usize) -> Self {
                    DisplayOffset(usize)
                }
            }

            impl From<CharOffset> for DisplayOffset {
                fn from(char_offset: CharOffset) -> Self {
                    DisplayOffset(char_offset.as_usize())
                }
            }

            impl<'a> Dimension<'a, TransformSummary> for DisplayOffset {
                fn add_summary(&mut self, summary: &'a TransformSummary) {
                    self.0 += &summary.display.chars.as_usize();
                }
            }

            impl<'a> Dimension<'a, TransformSummary> for Point {
                fn add_summary(&mut self, summary: &'a TransformSummary) {
                    *self += summary.buffer.lines;
                }
            }

            impl<'a> Dimension<'a, TransformSummary> for CharOffset {
                fn add_summary(&mut self, summary: &'a TransformSummary) {
                    *self += summary.buffer.chars;
                }
            }
        }
    }

    mod selections {
        use std::cmp::Ordering;
        use std::mem;
        use std::ops::Range;

        use pathfinder_geometry::vector::Vector2F;
        use serde::{Deserialize, Serialize};
        use string_offset::{ByteOffset, CharOffset};
        use vec1::Vec1;
        use warpui::AppContext;
        use warpui::text::point::Point;

        use super::DisplayPoint;
        use super::buffer::{Anchor, Buffer, ToBufferOffset, ToCharOffset, ToPoint};
        use super::display_map::{DisplayMap, ToDisplayPoint};
        use crate::editor::soft_wrap::{ClampDirection, DisplayPointAndClampDirection};
        use crate::editor::{CursorColors, RangeExt};

        /// This type encapsulates enough information about a selection to be able to
        /// draw it. Compared to the `Selection` type, the points are converted based on
        /// the `DisplayMap` to `DisplayPoint`s.
        pub struct DrawableSelection {
            pub range: Range<DisplayPoint>,
            pub clamp_direction: ClampDirection,
        }

        /// This type holds additional information about how to draw a local peer's
        /// selections and cursors.
        pub struct LocalDrawableSelectionData {
            pub colors: CursorColors,
            pub should_draw_cursors: bool,
        }

        /// The minimal set of data to identify a selection
        /// in the system.
        ///
        /// For local selections, we need more information
        /// so that we know how to extend them (see [`LocalSelection`]).
        #[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
        pub struct Selection {
            /// The start of the selection.
            /// The `start` is always <= the `end`,
            /// even if the selection is reversed.
            pub start: Anchor,
            pub end: Anchor,

            /// Whether or not the selection is reversed.
            /// If true, that means that the `start` is
            /// where the "head" of the selection is (i.e.
            /// where the cursor should be drawn, where the
            /// selection should be extended from, etc.).
            pub reversed: bool,
        }

        impl Selection {
            pub fn single_cursor(cursor: Anchor) -> Selection {
                Selection {
                    start: cursor.clone(),
                    end: cursor,
                    reversed: false,
                }
            }

            pub fn head(&self) -> &Anchor {
                if self.reversed {
                    &self.start
                } else {
                    &self.end
                }
            }

            pub fn tail(&self) -> &Anchor {
                if self.reversed {
                    &self.end
                } else {
                    &self.start
                }
            }

            pub fn range(&self, buffer: &Buffer) -> Range<Point> {
                let start = self.start.to_point(buffer).unwrap();
                let end = self.end.to_point(buffer).unwrap();
                if self.reversed {
                    end..start
                } else {
                    start..end
                }
            }

            pub fn display_range(&self, map: &DisplayMap, app: &AppContext) -> Range<DisplayPoint> {
                let start = self.start.to_display_point(map, app).unwrap();
                let end = self.end.to_display_point(map, app).unwrap();
                if self.reversed {
                    end..start
                } else {
                    start..end
                }
            }

            pub fn is_cursor_only(&self, buffer: &Buffer) -> bool {
                let start = self.start.to_point(buffer).unwrap();
                let end = self.end.to_point(buffer).unwrap();
                start == end
            }

            /// Whether the selection encompasses the entire buffer.
            pub fn spans_entire_buffer(&self, buffer: &Buffer) -> bool {
                self.range(buffer).sorted() == (Point::zero(), buffer.max_point())
            }

            pub fn to_offset(&self, buffer: &Buffer) -> Range<CharOffset> {
                let start = self
                    .head()
                    .to_char_offset(buffer)
                    .expect("Should be able to convert selection head to offset.");
                let end = self
                    .tail()
                    .to_char_offset(buffer)
                    .expect("Should be able to convert selection tail to offset.");
                start..end
            }

            pub fn to_byte_offset(&self, buffer: &Buffer) -> Range<ByteOffset> {
                let start = self
                    .head()
                    .to_byte_offset(buffer)
                    .expect("Should be able to convert selection head to offset.");
                let end = self
                    .tail()
                    .to_byte_offset(buffer)
                    .expect("Should be able to convert selection tail to offset.");
                start..end
            }

            pub fn start_to_point(&self, buffer: &Buffer) -> Point {
                self.start
                    .to_point(buffer)
                    .expect("Selection start should be valid Point")
            }

            pub fn end_to_point(&self, buffer: &Buffer) -> Point {
                self.end
                    .to_point(buffer)
                    .expect("Selection end should be valid Point")
            }

            /// Returns the selections display range iff it intersects the provided `range`.
            fn intersects_display_range(
                &self,
                range: &Range<DisplayPoint>,
                map: &DisplayMap,
                app: &AppContext,
            ) -> Option<Range<DisplayPoint>> {
                let display_range = self.display_range(map, app);
                // TODO (suraj): this check is confusing because [`Selection::display_range`]
                // possibly returns a reversed range.
                let intersects = display_range.start <= range.end || display_range.end <= range.end;
                intersects.then_some(display_range)
            }
        }

        /// A selection made by the client itself.
        /// Since the editor is CRDT-compliant, we distinguish
        /// between local and remote (peer) selections.
        ///
        /// A local selection contains enough information
        /// to identify the selection as well as _extend_ it.
        #[derive(Clone, PartialEq, Eq, Debug)]
        pub struct LocalSelection {
            pub selection: Selection,
            pub clamp_direction: ClampDirection,

            // goal_*_column's represent the columns this selection really should take.
            // Sometimes, `start`/`end` might be less than their goal_*_column counterparts
            // so we store these to use them once enough characters are present. For empty
            // selections, goal_start_column == goal_end_column, similar to start == end.
            //
            // As a concrete example, suppose we had the following buffer:
            // "words
            //  wo
            //  words2"
            // If the existing selection is ds on the first line, then if we add a cursor below,
            // it becomes an empty selection at the end of the line. Specifically, the buffer now looks like:
            // "wor<ds>|
            //  wo|
            //  words2"
            // where <x> denotes a selection of string x. Now, if we add one more cursor below,
            // we want the buffer to look like the following
            // "wor<ds>|
            //  wo|
            //  wor<ds>|2"
            // That is, if we didn't keep track of the goal start _and_ end columns,
            // we would not be able to correctly make the selection on the last row.
            // Specifically, the empty selection on the middle row had the following information stored:
            // {goal_start_column: 3, goal_end_column: 5}, which is how we were able to make the selection
            // correctly on the last row. If we only kept a single goal_column, we would either lose
            // information about the start of the selection or the end of the selection.
            pub goal_start_column: Option<u32>,
            pub goal_end_column: Option<u32>,
        }

        impl LocalSelection {
            pub fn start(&self) -> &Anchor {
                &self.selection.start
            }

            pub fn end(&self) -> &Anchor {
                &self.selection.end
            }

            pub fn head(&self) -> &Anchor {
                self.selection.head()
            }

            pub fn tail(&self) -> &Anchor {
                self.selection.tail()
            }

            pub fn reversed(&self) -> bool {
                self.selection.reversed
            }

            pub fn range(&self, buffer: &Buffer) -> Range<Point> {
                self.selection.range(buffer)
            }

            pub fn display_range(&self, map: &DisplayMap, app: &AppContext) -> Range<DisplayPoint> {
                self.selection.display_range(map, app)
            }

            pub fn is_cursor_only(&self, buffer: &Buffer) -> bool {
                self.selection.is_cursor_only(buffer)
            }

            /// Whether the selection encompasses the entire buffer.
            pub fn spans_entire_buffer(&self, buffer: &Buffer) -> bool {
                self.selection.spans_entire_buffer(buffer)
            }

            pub fn to_offset(&self, buffer: &Buffer) -> Range<CharOffset> {
                self.selection.to_offset(buffer)
            }

            pub fn to_byte_offset(&self, buffer: &Buffer) -> Range<ByteOffset> {
                self.selection.to_byte_offset(buffer)
            }

            pub fn start_to_point(&self, buffer: &Buffer) -> Point {
                self.selection.start_to_point(buffer)
            }

            pub fn end_to_point(&self, buffer: &Buffer) -> Point {
                self.selection.end_to_point(buffer)
            }

            pub fn set_start(&mut self, new_start: Anchor) {
                self.selection.start = new_start;
            }

            pub fn set_end(&mut self, new_end: Anchor) {
                self.selection.end = new_end;
            }

            pub fn set_reversed(&mut self, reversed: bool) {
                self.selection.reversed = reversed;
            }

            pub fn set_selection(&mut self, selection: Selection) {
                self.selection = selection;
            }

            pub fn set_head(&mut self, buffer: &Buffer, cursor: Anchor) {
                if cursor
                    .cmp(self.tail(), buffer)
                    .expect("Anchors should be comparable")
                    < Ordering::Equal
                {
                    if !self.selection.reversed {
                        mem::swap(&mut self.selection.start, &mut self.selection.end);
                        self.selection.reversed = true;
                    }
                    self.selection.start = cursor;
                } else {
                    if self.selection.reversed {
                        mem::swap(&mut self.selection.start, &mut self.selection.end);
                        self.selection.reversed = false;
                    }
                    self.selection.end = cursor;
                }
            }

            pub fn set_tail(&mut self, buffer: &Buffer, cursor: Anchor) {
                if cursor
                    .cmp(self.head(), buffer)
                    .expect("Anchors should be comparable")
                    > Ordering::Equal
                {
                    if !self.selection.reversed {
                        mem::swap(&mut self.selection.start, &mut self.selection.end);
                        self.selection.reversed = true;
                    }
                    self.selection.end = cursor;
                } else {
                    if self.selection.reversed {
                        mem::swap(&mut self.selection.start, &mut self.selection.end);
                        self.selection.reversed = false;
                    }
                    self.selection.start = cursor;
                }
            }
        }

        /// A ongoing selection made by the client itself.
        #[derive(Clone, PartialEq, Eq, Debug)]
        pub struct LocalPendingSelection {
            pub selection: LocalSelection,
            /// Determines how to extend selection based on starting selection.
            pub selection_mode: SelectionMode,
            /// Saved selection to refer back to when updating new selection.
            /// Used when extending selection from an already selected word.
            pub starting_selection: LocalSelection,
            pub is_single_selection: bool,
        }

        #[derive(Clone, Debug, PartialEq, Eq, Default)]
        pub enum MarkedTextState {
            /// There is currently marked text in the editor.
            Active {
                /// The current selection given by the IME within the marked text.
                selected_range: Range<usize>,
            },
            /// There is no marked text in the editor.
            #[default]
            Inactive,
        }

        /// The set of active and pending selections for this client.
        #[derive(Clone, PartialEq, Eq, Debug)]
        pub struct LocalSelections {
            /// A pending selection state, if any. This is used
            /// when a selection is actively being changed (e.g. mouse down).
            pub pending: Option<LocalPendingSelection>,

            /// The set of established selections.
            /// There is always at least one selection.
            pub selections: Vec1<LocalSelection>,

            /// Indicates whether or not the selections are marked text.
            pub marked_text_state: MarkedTextState,
        }

        impl LocalSelections {
            pub fn new(
                selections: Vec1<LocalSelection>,
                marked_text_state: MarkedTextState,
            ) -> Self {
                Self {
                    selections,
                    pending: None,
                    marked_text_state,
                }
            }

            pub fn first(&self) -> &LocalSelection {
                self.selections.first()
            }

            pub fn last(&self) -> &LocalSelection {
                self.selections.last()
            }

            pub fn selection_insertion_index(&self, start: &Anchor, buffer: &Buffer) -> usize {
                selection_insertion_index(&self.selections, start, buffer)
            }

            pub fn selections_intersecting_range<'a>(
                &'a self,
                range: Range<DisplayPoint>,
                map: &'a DisplayMap,
                app: &'a AppContext,
            ) -> impl Iterator<Item = (&'a LocalSelection, Range<DisplayPoint>)> + 'a {
                let pending_selection = self.pending.as_ref().and_then(|s| {
                    // If there's a single pending selection, it's already in the selections vector.
                    if s.is_single_selection {
                        return None;
                    }
                    let selection = &s.selection;
                    s.selection
                        .selection
                        .intersects_display_range(&range, map, app)
                        .map(|r| (selection, r))
                });

                selections_intersecting_range(&self.selections, range, map, app)
                    .chain(pending_selection)
            }

            pub fn drawable_selections_intersecting_range<'a>(
                &'a self,
                range: Range<DisplayPoint>,
                map: &'a DisplayMap,
                app: &'a AppContext,
            ) -> impl Iterator<Item = DrawableSelection> + 'a {
                self.selections_intersecting_range(range, map, app).map(
                    move |(selection, selection_range)| DrawableSelection {
                        range: selection_range,
                        clamp_direction: selection.clamp_direction,
                    },
                )
            }

            pub fn marked_text_state(&self) -> MarkedTextState {
                self.marked_text_state.clone()
            }

            pub fn set_marked_text_state(&mut self, marked_text_state: MarkedTextState) {
                self.marked_text_state = marked_text_state;
            }
        }

        impl From<Vec1<LocalSelection>> for LocalSelections {
            fn from(selections: Vec1<LocalSelection>) -> Self {
                Self {
                    selections,
                    pending: None,
                    marked_text_state: Default::default(),
                }
            }
        }

        /// Determines how to extend selection for text.
        /// Either extending by characters, lines or words at a time.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum SelectionMode {
            Chars,
            Lines,
            Words,
        }

        #[derive(Debug)]
        pub enum SelectAction {
            Begin {
                position: DisplayPointAndClampDirection,
                add: bool,
            },
            Update {
                position: DisplayPoint,
                scroll_position: Vector2F,
            },
            Extend {
                position: DisplayPoint,
                scroll_position: Vector2F,
            },
            End,
        }

        impl SelectAction {}

        pub trait AsSelection {
            fn as_selection(&self) -> &Selection;
            fn as_mut_selection(&mut self) -> &mut Selection;
        }

        impl AsSelection for LocalSelection {
            fn as_selection(&self) -> &Selection {
                &self.selection
            }

            fn as_mut_selection(&mut self) -> &mut Selection {
                &mut self.selection
            }
        }

        fn selection_insertion_index<S: AsSelection>(
            selections: &Vec1<S>,
            start: &Anchor,
            buffer: &Buffer,
        ) -> usize {
            match selections
                .binary_search_by(|probe| probe.as_selection().start.cmp(start, buffer).unwrap())
            {
                Ok(index) => index,
                Err(index) => {
                    if index > 0
                        && selections[index - 1]
                            .as_selection()
                            .end
                            .cmp(start, buffer)
                            .unwrap()
                            == Ordering::Greater
                    {
                        index - 1
                    } else {
                        index
                    }
                }
            }
        }

        /// Returns an iterator over the selections
        /// that intersect the given `range`.
        ///
        /// Assumes that `selections` are sorted by their start point.
        fn selections_intersecting_range<'a, S: AsSelection>(
            selections: &'a Vec1<S>,
            range: Range<DisplayPoint>,
            map: &'a DisplayMap,
            app: &'a AppContext,
        ) -> impl Iterator<Item = (&'a S, Range<DisplayPoint>)> + 'a {
            let start = map
                .anchor_before(range.start, super::Bias::Left, app)
                .unwrap();
            let start_index = selection_insertion_index(selections, &start, map.buffer(app));

            selections[start_index..]
                .iter()
                .map_while(move |selection| {
                    selection
                        .as_selection()
                        .intersects_display_range(&range, map, app)
                        .map(|r| (selection, r))
                })
        }
    }
}

mod soft_wrap {
    use std::sync::Arc;

    use anyhow::anyhow;
    use parking_lot::Mutex;
    use warpui::text_layout;

    use crate::editor::DisplayPoint;
    use crate::editor::Point;

    #[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
    pub struct SoftWrapPoint(Point);

    impl SoftWrapPoint {
        pub fn new(row: u32, col: u32) -> Self {
            Self(Point::new(row, col))
        }

        pub fn row(self) -> u32 {
            self.0.row
        }

        pub fn column(self) -> u32 {
            self.0.column
        }

        pub fn column_mut(&mut self) -> &mut u32 {
            &mut self.0.column
        }
    }

    /// When a line is soft-wrapped, there can be ambiguity about where the cursor
    /// should be drawn.  For example, if I had the text "hello world" which became
    /// soft wrapped to "hello \nworld", then if the cursor is just before "w",
    /// then the cursor could either be at the end of the first line or the very
    /// beginning of the next line. This field keeps track of whether the cursor
    /// should be clamped above or below.
    #[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
    pub enum ClampDirection {
        Up,
        #[default]
        Down,
    }

    #[derive(Default, Clone)]
    /// Wrapper around `FrameLayouts` to help manage the levels of indirection around
    /// the Arc, Mutex and Option.
    ///
    /// When this is None, we haven't yet laid out the text.
    pub struct SoftWrapState(Arc<Mutex<Option<FrameLayouts>>>);

    impl SoftWrapState {
        pub fn update(&self, new_value: FrameLayouts) {
            *self.0.lock() = Some(new_value);
        }

        pub fn read<T, F: FnOnce(anyhow::Result<&FrameLayouts>) -> T>(&self, callback: F) -> T {
            let frame_layouts = self.0.lock();
            match &*frame_layouts {
                Some(frame_layouts) => callback(Ok(frame_layouts)),
                None => callback(Err(anyhow!(
                    "No frame layout. This should only happen if we attempt to read before a frame of the editor has been laid out"
                ))),
            }
        }
    }

    /// We layout more frames than we need to display on the screen because
    /// we need to know the number of lines in previous frames to calculate
    /// any row position for any given character.
    ///
    /// In particular, we always layout from zero up to the number of lines in
    /// viewport + number of lines in scrollback because this is the worst case
    /// and we don't know how many rows we'll need to be laid out until we've laid
    /// them out.
    #[derive(Clone)]
    pub struct FrameLayouts {
        frames: Vec<Arc<text_layout::TextFrame>>,
        /// The first line that is displayed. The very first line in the editor
        /// is 0, and each displayed line is incremented by 1. So if soft wrapping
        /// is off, this is one-to-one with the rows of the buffer, but if soft
        /// wrapping is on, the line number is the number of lines from the top
        /// given soft wrapping.
        start_line: u32,
        end_line: u32,
    }

    impl FrameLayouts {
        pub fn new(
            frames: Vec<Arc<text_layout::TextFrame>>,
            start_line: u32,
            end_line: u32,
        ) -> Self {
            Self {
                frames,
                start_line,
                end_line,
            }
        }

        pub fn frames(&self) -> impl Iterator<Item = &Arc<text_layout::TextFrame>> {
            self.frames.iter()
        }

        pub fn last_frame(&self) -> Option<&Arc<text_layout::TextFrame>> {
            self.frames.last()
        }

        pub fn displayed_lines(&self) -> impl Iterator<Item = &text_layout::Line> {
            FrameLayoutDisplayedLines::zero(self).skip(self.start_line as usize)
        }

        /// Number of lines displayed in the editor.
        pub fn num_displayed_lines(&self) -> u32 {
            self.displayed_lines().count() as u32
        }

        pub fn num_lines(&self) -> usize {
            self.frames().map(|frame| frame.lines().len()).sum()
        }

        /// Returns the index of the row that represents the end of some logical row
        pub fn end_of_logical_row(&self, logical_row: usize) -> usize {
            self.frames
            .iter()
            // adding 1 to include the TextFrame that represents this row (the queried one)
            .take(logical_row + 1)
            .fold(0, |acc, text_frame| acc + text_frame.lines().len())
            // subtracting one to convert a count of rows into an index
            - 1
        }

        /// Returns the line at the given index, where the first line is 0 if it exists.
        pub fn get_line(&self, desired_index: usize) -> Option<&text_layout::Line> {
            self.frames
                .iter()
                .flat_map(|frame| frame.lines())
                .nth(desired_index)
        }

        /// Given a `DisplayPoint`, converts the row number such that soft wrapping
        /// is considered. This function will yield a result in the case where the
        /// provided point is within the bounds of the last laid out text. Otherwise,
        /// it will return `None`.
        pub fn to_soft_wrap_point(
            &self,
            point: DisplayPoint,
            clamp_direction: ClampDirection,
        ) -> Option<SoftWrapPoint> {
            let rows_before_frame: u32 = self
                .frames
                .iter()
                .take(point.row() as usize)
                .fold(0, |acc, text_frame| acc + text_frame.lines().len() as u32);

            let frame = self.frames.get(point.row() as usize)?;

            let rows_within_frame = frame.row_within_frame(
                point.column() as usize,
                matches!(clamp_direction, ClampDirection::Up),
            ) as u32;
            Some(SoftWrapPoint::new(
                rows_before_frame + rows_within_frame,
                point.column(),
            ))
        }

        /// Returns the `[start, end)` display columns of the visual (soft-wrapped)
        /// row that `point` lies on. The columns are in `DisplayPoint` column
        /// coordinates (i.e. the same coordinate space as `DisplayPoint::column`).
        /// Returns `None` if the point is outside the bounds of the laid out text.
        pub fn soft_wrapped_row_bounds(
            &self,
            point: DisplayPoint,
            clamp_direction: ClampDirection,
        ) -> Option<std::ops::Range<u32>> {
            let soft_wrap_point = self.to_soft_wrap_point(point, clamp_direction)?;
            let line = self.get_line(soft_wrap_point.row() as usize)?;
            // If the row has no glyphs we can't derive column bounds from the layout
            // (this happens for un-laid-out / fontless frames). Return `None` so the
            // caller falls back to logical-line behavior.
            let first_glyph_index = line.first_glyph()?.index as u32;
            // Derive the row's column bounds from its caret positions when present.
            // A single caret position can span multiple buffer characters (emoji,
            // ligatures, and other shaped clusters), so the row end must be one past
            // the last caret position's final character; using `last_glyph().index +
            // 1` would stop *inside* the final cluster. Fall back to glyph indices
            // for lines that carry no caret positions (e.g. the `mock` lines used in
            // unit tests).
            let start = line
                .caret_positions
                .first()
                .map_or(first_glyph_index, |caret| caret.start_offset as u32);
            let end = line
                .caret_positions
                .last()
                .map(|caret| caret.last_offset as u32 + 1)
                .or_else(|| line.last_glyph().map(|glyph| glyph.index as u32 + 1))
                .unwrap_or(start);
            Some(start..end)
        }

        /// Given a `SoftWrapPoint`, converts the row number to a value irrespective
        /// of soft-wrapping.
        pub fn to_display_point(&self, point: SoftWrapPoint) -> DisplayPointAndClampDirection {
            let mut acc_lines = 0;
            let mut row = 0;
            let mut clamp_direction = Default::default();
            for (i, frame) in self.frames.iter().enumerate() {
                if acc_lines + frame.lines().len() > point.row() as usize {
                    row = i;
                    break;
                }
                acc_lines += frame.lines().len()
            }
            if let Some(line) = self.get_line(point.row() as usize) {
                if let Some(glyph) = line.last_glyph() {
                    if point.column() as usize == glyph.index + 1 {
                        clamp_direction = ClampDirection::Up;
                    }
                } else if let Some(glyph) = line.first_glyph()
                    && point.column() as usize == glyph.index
                {
                    clamp_direction = ClampDirection::Down;
                }
            }

            DisplayPointAndClampDirection {
                point: DisplayPoint::new(row as u32, point.column()),
                clamp_direction,
            }
        }
    }

    #[derive(Debug)]
    pub struct DisplayPointAndClampDirection {
        pub point: DisplayPoint,
        pub clamp_direction: ClampDirection,
    }

    pub struct FrameLayoutDisplayedLines<'a> {
        frames: &'a FrameLayouts,
        current_row: usize,
        current_line_of_row: usize,
        lines_yielded: u32,
    }

    impl<'a> FrameLayoutDisplayedLines<'a> {
        pub fn zero(frames: &'a FrameLayouts) -> Self {
            Self {
                frames,
                current_row: 0,
                current_line_of_row: 0,
                lines_yielded: 0,
            }
        }
    }

    impl<'a> Iterator for FrameLayoutDisplayedLines<'a> {
        type Item = &'a text_layout::Line;

        fn next(&mut self) -> Option<Self::Item> {
            if self.lines_yielded >= self.frames.end_line {
                return None;
            }
            if let Some(frame) = self.frames.frames.get(self.current_row) {
                if frame.lines().len() > self.current_line_of_row {
                    let line = &frame.lines()[self.current_line_of_row];
                    self.current_line_of_row += 1;
                    self.lines_yielded += 1;
                    Some(line)
                } else {
                    self.current_row += 1;
                    self.current_line_of_row = 0;
                    self.next()
                }
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
#[path = "EditorView_tests.rs"]
mod tests;
