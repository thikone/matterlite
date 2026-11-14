# EditorView

`crates/ml_editor/src/EditorView.rs` is Warp's text editor view, extracted from
`warp-terminal/app/src/editor` @ `5bb7bcc6a` into one file. It provides single-line inputs
(`EditorView::single_line`) and multi-line inputs (`EditorView::new`). This document lists what
the file contains, how big each part is, where each part came from in Warp, what it did there,
and where it fits in MatterLite's UI.

Line counts are for the file as of this document (15,508 lines) and include doc comments.
Counts for groups of methods are sums of the methods' own lines, so they don't add up exactly
to the section totals (blank lines and small helpers are left out).

Related files:

| File | Lines | Contents |
|---|---:|---|
| `crates/ml_editor/src/EditorView.rs` | 15,508 | everything below |
| `crates/ml_editor/src/EditorView_tests.rs` | 231 | 7 smoke tests (typing, enter handling, newlines, undo/redo, limits, placeholder) |
| `crates/ml_editor/src/lib.rs` | 9 | loads `EditorView.rs` as module `editor` and re-exports it |

## Contents

1. [File layout](#1-file-layout)
2. [Features and their size](#2-features-and-their-size)
3. [Public interface](#3-public-interface)
4. [Where to use it in MatterLite](#4-where-to-use-it-in-matterlite)
5. [Not included](#5-not-included)

---

## 1. File layout

The file follows Warp's module tree. Warp's submodule files are inline `mod … { }` blocks.

| Lines | Size | Section | Warp source | Role |
|---|---:|---|---|---|
| 1–80 | 80 | Header, imports, re-exports, constants | `app/src/editor/view/mod.rs` | |
| 81–693 | 613 | `init`: keybindings, settings registration | `view/mod.rs` `init` | Default shortcuts |
| 695–1214 | 520 | Action enum and option types | `view/mod.rs` | `EditorAction`, `EditorOptions`, `SingleLineEditorOptions`, `TextOptions`, `EnterSettings`, … |
| 1216–1322 | 107 | `struct EditorView`, `ScrollState` | `view/mod.rs` | View state |
| 1324–4537 | 3,214 | `impl EditorView` | `view/mod.rs` | All editor commands and queries |
| 4539–4880 | 342 | `Event`, `TypedActionView`, `View`, position ids | `view/mod.rs` | Action dispatch, rendering entry point, focus handling |
| 4882–4932 | 51 | `CursorBlink`, `CursorDisplayType` | `app/src/settings/editor.rs` | Cursor setting values |
| 4938–4978 | 41 | `mod editor_settings` | `app/src/settings/editor.rs` (`AppEditorSettings`) | Cursor and auto-close settings |
| 4980–5066 | 87 | `mod selection_settings` | `app/src/settings/select.rs` | Linux primary selection, middle-click paste |
| 5068–5238 | 171 | Helpers | `app/src/util/clipboard.rs`, `util/mod.rs`, `util/extensions.rs`, `terminal/grid_size_util.rs`, `editor/mod.rs` | Clipboard text, range helpers, grid baseline |
| 5241–6863 | 1,623 | `mod element` | `view/element.rs` | Layout, painting, mouse and IME events |
| 6865–7331 | 467 | `mod snapshot` | `view/snapshot.rs` | Read-only view state for rendering, text layout, autoscroll |
| 7333–7369 | 37 | `mod movement` | `view/movement.rs` | Left/right movement on the display map |
| 7371–15220 | 7,850 | `mod model` | `view/model/` | Everything below |
| 7371–9192 | 1,822 | model (own code) | `view/model/mod.rs` | Edit transactions, selections, insertion rules |
| 9194–13627 | 4,434 | `mod buffer` | `view/model/buffer/` | CRDT text buffer (own code 3,168 lines plus the submodules below) |
| 12357–12447 | 91 | `mod anchor` | `buffer/anchor.rs` | Positions that survive edits |
| 12449–12759 | 311 | `mod subword_boundaries` | `buffer/subword_boundaries.rs` | camelCase / snake_case word parts |
| 12761–13165 | 405 | `mod text` | `buffer/text.rs` | Text chunks with line/char/byte summaries |
| 13167–13354 | 188 | `mod time` | `buffer/time.rs` | Lamport clocks, version vectors, replica ids |
| 13356–13626 | 271 | `mod undo` | `buffer/undo.rs` | Undo stack and undo history |
| 13629–14694 | 1,066 | `mod display_map` | `view/model/display_map/` | Buffer → display coordinates, tabs (own code 547 lines) |
| 14175–14693 | 519 | `mod fold_map` | `display_map/fold_map.rs` | Folded ranges (kept as plumbing; no fold commands) |
| 14696–15219 | 524 | `mod selections` | `view/model/selections.rs` | Selection types, pending (drag) selection |
| 15222–15503 | 282 | `mod soft_wrap` | `app/src/editor/soft_wrap.rs` | Soft-wrapped rows ↔ display points |
| 15505–15507 | 3 | `mod tests` | | Includes `EditorView_tests.rs` |

---

## 2. Features and their size

In Warp, all of this served one main purpose: the **terminal command input**, where you type
shell commands and AI prompts. The same view was also used for smaller fields such as the
settings search, rename dialogs, the find bar and the font-size field.

### 2.1 Text storage — `mod buffer` (4,434 lines)

Warp source: `app/src/editor/view/model/buffer/`.

| Feature | Lines | What it does | Purpose in Warp |
|---|---:|---|---|
| CRDT fragment tree | ~1,100 | Text is a `sum_tree` of fragments, each tagged with the insertion that created it and the edits that deleted it. `update_fragments_for_edit` alone is 335 lines. | Lets several people edit the same input in shared sessions. In MatterLite it's just the storage; nobody else sends edits. |
| Batching | ~300 | Every change happens inside a batch (`start_batch`/`end_batch`, `BatchState`, 128 lines), which records undo entries and emits `Edited` / `SelectionsChanged` once. | One event per user action. |
| Undo / redo | 271 (`undo`) + ~100 | Local undo stack. Typing within 500 ms is undone as one step; paste, newline, word deletion and similar "atomic" actions undo separately. Selections are restored with the text. | Undo in the command input. |
| Styled text | ~600 | `TextStyleOperation` sets or clears foreground, syntax, background and error-underline colours on ranges. Styles survive edits and undo. | Syntax highlighting of shell commands and error underlines. |
| Anchors | 91 + ~115 | Positions that stay attached to text while edits happen around them. Selections are stored as anchors. | Stable cursors and selections. |
| Text queries | ~350 | Lines, line lengths, chars, styled chars, text runs, byte/char/point conversions (`ToCharOffset`, `ToBufferOffset`, `ToPoint`). | |
| Word and subword boundaries | 311 | Iterates word starts and ends, and camelCase / snake_case parts. | Alt+arrow and ctrl-alt+arrow movement. |
| Clocks and versions | 188 | Lamport clocks, version vectors, replica ids (now a process-local counter). | Ordering edits between peers. |
| Text chunks | 405 | Text pieces with summaries (lines, chars, bytes). | |

### 2.2 Editor model — model's own code (1,822 lines)

Warp source: `app/src/editor/view/model/mod.rs`.

| Feature | Lines | What it does | Purpose in Warp |
|---|---:|---|---|
| Edit transactions | ~320 | `Edits` builder: change selections, update the buffer with an action and an `EditOrigin`, undo options (`SkipUndoRedoRecord`, `IsEphemeral`). `EditorModel::edit` (101 lines) runs them. | Every edit from the view goes through this. |
| Temporary ("ephemeral") text | ~126 | A second buffer shows text without touching the real one or the undo stack. The first real edit applies it. | Previewing a history entry or a model choice in the input. |
| Insertion rules | ~260 | Insert at every cursor, auto-close `( [ { ' "`, type over an existing closing character, wrap a selection in brackets or quotes. | Typing in the command input. |
| Input limits | 24 | Rejects text that would exceed `max_buffer_len` or break `ValidInputType`. | Numeric settings fields, length-limited names. |
| Selection operations | ~250 | Select ranges by offset or display point, select words left and right, word start/end, line start/end, move every cursor with a function. | |
| Deletion | ~100 | Backspace, delete, remove around the cursor (deleting an empty `()` pair), cut to the internal clipboard. | |
| IME marked text | 61 | Tracks the IME composition and its selected range, and commits it. | Japanese, Chinese and Korean input. |
| Indent / unindent | 30 | | Tab and Shift-Tab in multi-line input. |
| Accessibility | 77 | Describes what changed (typed characters, "Selected: …", "Unselected") for screen readers. | |
| Undo / redo plumbing | 35 | | |

### 2.3 Display — `display_map`, `fold_map`, `soft_wrap`, `movement`, `selections` (2,428 lines)

| Feature | Lines | Warp source | What it does |
|---|---:|---|---|
| Display map | 547 | `model/display_map/mod.rs` | Converts buffer points to display points, expands tabs, moves up and down while keeping the goal column. |
| Fold map | 519 | `display_map/fold_map.rs` | Folded ranges between buffer and display. No commands create folds any more, but the display map is built on it. |
| Soft wrap | 282 | `editor/soft_wrap.rs` | Maps between logical rows and wrapped visual rows, using the laid-out text frames. |
| Movement | 37 | `view/movement.rs` | Left and right movement on display points. |
| Selections | 524 | `model/selections.rs` | `Selection`, `LocalSelection` (with goal columns and clamp direction), `LocalPendingSelection` (mouse drag by chars, words or lines), `SelectAction`, `MarkedTextState`. |

### 2.4 Rendering — `mod element` (1,623 lines) and `mod snapshot` (467 lines)

Warp source: `app/src/editor/view/element.rs`, `snapshot.rs`.

| Feature | Lines | What it does |
|---|---:|---|
| Layout and paint | ~525 | `layout` (147) lays out text frames, the placeholder and decorators, and computes the height for autogrow. `paint` (163) draws decorators, selections, cursors and text, and caches positions. Also `after_layout` (39), `LayoutState` (55) and hit-testing in `PaintState` (121). |
| Selections and cursors | ~330 | Selection highlight per row, including a small tick where a selection crosses a newline. Bar, block or underline cursors with the theme's colours. IME composition is drawn as a selection. |
| Text and placeholder lines | ~110 | Paints each line with the computed baseline, and the placeholder after the last line. |
| Mouse, keyboard and IME events | ~335 | Click, double and triple click, shift-click, alt/cmd-click for another cursor, drag with autoscroll, middle-click, wheel and trackpad scrolling, typed characters, IME events. |
| Decorators | ~70 | Elements above (top section), top-left (left notch) and top-right (right notch) of the text. Text on the first line starts after the left notch. |
| Snapshot | 467 | A copy of everything rendering needs (font, metrics, placeholders, cached points). Text frames are laid out in parallel with `rayon` (107). Autoscroll keeps the cursor visible (139). Placeholder layout (100). |

In Warp this drew the command input with its prompt in the decorators (multi-line PS1 above, the
last prompt line on the left, the right prompt on the right), plus remote cursors with avatars
and autosuggestion hint chips. The last two are removed.

### 2.5 View — `impl EditorView` (3,214 lines)

Warp source: `app/src/editor/view/mod.rs`.

| Feature | Lines | Methods (main ones) |
|---|---:|---|
| Cursor movement and keyboard selection | 823 | `move_left/right`, `up/down`, `move_up/down`, `select_up/down/left/right`, word and subword movement, line, visual-line, paragraph and buffer start/end, `page_up/down`, `select_all` |
| Mouse selection | 325 | `begin_selection`, `update_selection` (117: drag by chars, words or lines), `extend_selection`, `end_selection`, `select_word`, `select_line`, `scroll` |
| Setting content from code | 284 | `set_buffer_text`, `system_reset_buffer_text`, `set_buffer_text_ignoring_undo`, `clear_buffer`, `system_insert`, `system_delete`, `select_and_replace`, `replace_first/last_n_characters`, `insert_selected_text` |
| Multiple cursors | 228 | `add_cursor` (above/below), `add_next_occurrence`, `expand_selections` |
| Deletion commands | 219 | word left/right (cut or delete), cut/delete all left/right, `clear_and_copy_lines`, `clear_lines` |
| Typing, Enter, Tab, Escape | 210 | `user_insert`, `enter`, `shift_enter`, `alt_enter`, `ctrl_enter`, `newline`, `backspace`, `delete`, `handle_tab`, `unindent`, `escape` |
| Construction and settings | 185 | `single_line`, `new`, `new_internal`, settings and window-focus subscriptions, `snapshot` |
| Edge queries and key propagation | 140 | `single_cursor_on_first_row/line`, `…_on_last_row/line`, `…_at_buffer_start/end`, `single_cursor_line_index`, and whether up, down, left or right goes to the parent |
| Reading content | 126 | `buffer_text`, `is_empty`, `is_dirty`, `selected_text`, byte offsets of selections, `snapshot_model`, `num_selections` |
| Clipboard | 78 | `copy`, `cut`, `paste`, middle-click paste, `yank` |
| Styles | 78 | `update_buffer_styles`, `insert_with_styles`, `text_style_runs`, `clear_text_style_runs` |
| Font and metrics | 71 | `font_size`, `font_family`, `line_height`, `em_width`, `set_font_family`, `set_font_size`, `set_text_colors` |
| IME | 63 | marked text, commit, commit-on-blur |
| Cursor blinking | 56 | 500 ms blink, only when focused in the active window |
| Configuration | 51 | `set_autogrow`, `set_enter_settings`, propagation setters, `set_interaction_state`, `set_autocomplete_symbols_allowed` |
| Placeholders | 35 | default and per-prefix placeholders |
| Popup positions | 18 | `cache_buffer_point` |
| Undo / redo | 10 | `undo`, `redo` |

### 2.6 Input wiring and settings

| Feature | Lines | Warp source | What it does |
|---|---:|---|---|
| Keybindings | 613 | `view/mod.rs` `init` | Default shortcuts, see [3.6](#36-keybindings) |
| Action dispatch | 170 | `view/mod.rs` `TypedActionView` | Maps `EditorAction` to methods; screen-reader text for actions; writes selections to the Linux primary selection |
| View trait | 102 | `view/mod.rs` `View` | `render`, keymap context, focus and blur, IME cursor position |
| Editor settings | 92 | `app/src/settings/editor.rs` | `cursor_blink`, `cursor_display_type`, `autocomplete_symbols` |
| Selection settings | 87 | `app/src/settings/select.rs` | `linux_selection_clipboard`, `middle_click_paste_enabled` |
| Grid baseline | 91 | `app/src/terminal/grid_size_util.rs` | Line height and baseline from font metrics, so text lines up with a character grid |
| Clipboard text, ranges | 80 | `util/clipboard.rs`, `util/mod.rs`, `util/extensions.rs`, `editor/mod.rs` | Clipboard text (file paths joined by spaces), `RangeExt`, `merge_ranges`, `SliceExt` |

---

## 3. Public interface

From the crate entry point down to the data types. Everything is re-exported at the crate
root (`ml_editor::EditorView`, `ml_editor::Event`, …).

### 3.1 Entry points

```rust
ml_editor::init(ctx: &mut AppContext)
```
Call once at startup, after the settings stores and `Appearance` exist. It registers the
keybindings and the `EditorSettings` and `SelectionSettings` groups. Creating an editor before
`init` panics on the first settings read.

```rust
EditorView::single_line(options: SingleLineEditorOptions, ctx: &mut ViewContext<EditorView>) -> EditorView
EditorView::new(options: EditorOptions, ctx: &mut ViewContext<EditorView>) -> EditorView
```
`EditorView` is a WarpUI `View` (`ui_name` `"EditorView"`) and `TypedActionView` (action
`EditorAction`). Add it with `ctx.add_typed_action_view(|ctx| EditorView::single_line(…, ctx))`,
render it with `ChildView`, and subscribe to its `Event`s.

### 3.2 Options

**`SingleLineEditorOptions`** (all fields `pub`, `Default` provided):

| Field | Default | Meaning |
|---|---|---|
| `text: TextOptions` | theme defaults | font size, family, properties and colour overrides |
| `enter_settings: EnterSettings` | Enter emits | what Enter, Shift/Alt/Ctrl+Enter do |
| `propagate_and_no_op_vertical_navigation_keys` | `Never` | hand Up/Down and PageUp/PageDown to the parent: `Always`, `AtBoundary` (cursor on the first/last row), `Never`. With `Always`, Tab and Shift+Tab are handed over too |
| `propagate_horizontal_navigation_keys` | `Never` | hand Right to the parent: `Always`, `AtBoundary` (cursor at the end), `Never`. Left is never handed over |
| `use_settings_line_height_ratio` | `false` | use Appearance's line height instead of the UI default |
| `autocomplete_symbols` | `false` | auto-close brackets and quotes (also needs the user setting) |
| `soft_wrap`, `placeholder_soft_wrap` | `false` | wrap long text / placeholder |
| `select_all_on_focus` | `false` | |
| `clear_selections_on_blur` | `false` | |
| `max_buffer_len: Option<usize>` | `None` | maximum length in characters |
| `valid_input_type: ValidInputType` | `All` | `All`, `PositiveInteger` (must parse as `u16`), `NoSpaces` |
| `baseline_position_computation_method` | `Default` | `Default` or `Grid` (font-metric baseline) |
| `middle_click_paste` | `true` | set `false` if the parent already handles middle-click |
| `allow_user_cursor_preference` | `false` | use the user's bar/block/underline setting |
| `convert_newline_to_space` | `true` | pasted newlines become spaces |
| `is_password` | `false` | draw dots, disable copy and cut |

**`EditorOptions`** has the same fields (with `convert_newline_to_space` defaulting to `false`)
plus:

| Field | Meaning |
|---|---|
| `cursor_colors_fn: CursorColorsFn` | `Fn(&AppContext) -> CursorColors`; default `default_cursor_colors` (theme cursor and selection colours) |
| `autogrow: bool` | grow with the content up to the parent's maximum height |
| `single_line: bool` | set by `single_line`; Enter never inserts a newline |
| `render_decorator_elements: Option<Box<dyn Fn(&AppContext) -> EditorDecoratorElements>>` | elements above, top-left and top-right of the text |
| `delegate_paste_handling: bool` | don't insert clipboard text; only emit `Event::Paste` |
| `keymap_context_modifier: Option<KeymapContextModifierFn>` | add flags to the editor's keymap context |

Supporting types:

- `TextOptions { font_size_override, font_family_override, font_properties_override, text_colors_override }`,
  with `TextOptions::ui_font_size(appearance)` and `TextOptions::ui_text(size, appearance)`.
- `EnterSettings { enter, shift_enter, alt_enter, ctrl_enter }` of `EnterAction::Emit` or
  `EnterAction::InsertNewLineIfMultiLine`. Default: Enter emits, the others insert a newline.
- `TextColors { default_color, disabled_color, hint_color }` (`Fill`), with
  `TextColors::from_appearance` and `TextColors::all_hint_color`.
- `CursorColors { cursor, selection }` (`Fill`).
- `EditorDecoratorElements { top_section, left_notch, right_notch, right_notch_offset_px }`. The
  left notch must be exactly one line high.

### 3.3 Events (`ml_editor::Event`)

| Event | When |
|---|---|
| `Edited(EditOrigin)` | text changed; `EditOrigin::is_user()` tells typing apart from code changes |
| `SelectionChanged` | cursors or selections moved without an edit |
| `Focused` | the editor was clicked (programmatic focus doesn't emit it) |
| `Blurred` | the editor lost focus |
| `Activate` | clicked while not focused |
| `Enter`, `ShiftEnter`, `AltEnter`, `CtrlEnter`, `CmdEnter` | per `EnterSettings`; a single-line editor emits `ShiftEnter` for Shift+Enter and ctrl-j |
| `Escape` | Esc with a single cursor (with several cursors, Esc collapses them first) |
| `Navigate(NavigationKey)` | Up, Down, PageUp, PageDown, Right, Tab or Shift+Tab handed to the parent per the propagation options |
| `CmdUpOnFirstRow` | Cmd+↑ (Ctrl+Home on Windows/Linux) with the cursor already at the very start |
| `BackspaceOnEmptyBuffer` | Backspace in an empty editor |
| `BackspaceAtBeginningOfBuffer` | Backspace with the cursor at position 0 |
| `Copy` | copy with nothing selected (the parent can copy something else) |
| `Paste`, `MiddleClickPaste` | after a paste (or instead of it, with `delegate_paste_handling`) |
| `DeleteAllLeft` | cmd-backspace on macOS |
| `BufferReplaced` | the whole text was replaced |
| `ClearParentSelections` | a click in the editor; the parent can clear its own selection |

### 3.4 Methods

**Content**

| Method | Undo | Notes |
|---|---|---|
| `buffer_text(ctx) -> String`, `is_empty`, `buffer_size` | | |
| `set_buffer_text(&str, ctx)` | recorded | works even when the editor is disabled |
| `system_reset_buffer_text(&str, ctx)` | stack cleared | for content synced from elsewhere |
| `set_buffer_text_with_base_buffer(&str, ctx)` | recorded | also sets the base text for `is_dirty` |
| `set_buffer_text_ignoring_undo(&str, ctx)`, `insert_selected_text_to_buffer_ignoring_undo` | not recorded | temporary text, applied by the next real edit |
| `clear_buffer`, `system_clear_buffer(reset_undo_stack, ctx)`, `clear_buffer_and_reset_undo_stack` | | |
| `user_insert(&str, ctx)` | recorded | as if typed (auto-close applies) |
| `user_initiated_insert`, `system_insert(&str, PlainTextEditorViewAction, ctx)` | recorded | at the cursors, with an undo action type |
| `insert_selected_text(&str, ctx)` | recorded | inserted text stays selected |
| `select_and_replace(&str, ranges, action, ctx)` | recorded | replace byte ranges |
| `replace_first_n_characters(n, &str, ctx)`, `replace_last_n_characters` | recorded | |
| `system_delete(Range<ByteOffset>, ctx)` | recorded | |
| `set_base_buffer_text(String, ctx)`, `is_dirty(ctx)` | | dirty = differs from the base text |
| `last_buffer_text(ctx)`, `get_last_action(ctx)` | | text before the last edit, last action type |
| `snapshot_model(ctx) -> EditorSnapshot` | | text and selections as char ranges |

**Selections and cursor**

- Read: `selected_text`, `selected_text_strings`, `num_selections`, `is_single_cursor_only`,
  `any_selections_span_entire_buffer`, `chars_preceding_selections`, `is_selecting`,
  `start_byte_index_of_first_selection`, `start_byte_index_of_last_selection`,
  `end_byte_index_of_last_selection`, `start_byte_offset_at_point`.
- Edge queries: `single_cursor_to_point`, `first_selection_end_to_point`,
  `single_cursor_at_buffer_start/end`, `single_cursor_on_first_row/line`,
  `single_cursor_on_last_row/line` (row = visual wrapped row, line = logical line),
  `single_cursor_line_index`.
- Change: `select_ranges_by_byte_offset`, `reset_selections_to_point`, `select_all`,
  `select_word`, `select_line`, `clear_selections`, `add_next_occurrence`.
- Movement: `move_left/right(stop_at_line_edge)`, `up`, `down`, `right`, `move_up/down`,
  `select_up/down/left/right`, `move_to_line_start/end`, `move_to_visual_line_start/end`,
  `move_to_paragraph_start/end`, `move_to_buffer_start/end`,
  `move_and_select_to_buffer_start/end`, `select_to_line_start/end`, `cursor_top`,
  `cursor_bottom`, `cmd_up`, `page_up/down`, `move_page_up/down`.

**Editing commands:** `backspace`, `delete`, `cut_word_left/right`, `delete_word_left/right`,
`clear_and_copy_lines`, `clear_lines`, `handle_tab`, `unindent`, `escape`, `copy`, `cut`,
`paste`, `undo`, `redo`.

**Styling:** `update_buffer_styles(ranges, TextStyleOperation, ctx)`,
`insert_with_styles(&str, &[(Range<ByteOffset>, TextStyle)], action, ctx)`,
`text_style_runs(ctx)`, `clear_text_style_runs(ctx)`.

**State and configuration:** `is_single_line`, `is_focused`, `can_edit`, `can_select`,
`interaction_state`, `set_interaction_state(InteractionState)` (`Editable`,
`EditableWithInvalidSelection`, `Selectable`, `Disabled`), `set_autogrow`,
`set_enter_settings`, `enter_settings`, `set_propagate_vertical_navigation_keys`,
`set_propagate_horizontal_navigation_keys`, `set_autocomplete_symbols_allowed`.

**Placeholders:** `set_placeholder_text(text)`, `set_placeholder_text_with_prefix(prefix, text)`
(shown when the text equals `prefix`), `clear_placeholder_text`,
`clear_placeholder_text_with_prefix`, `clear_all_placeholder_text`, `placeholder_text_exists`.

**Appearance and metrics:** `set_font_family`, `set_font_size`, `set_text_colors`,
`font_size(appearance)`, `font_family`, `placeholder_font_family`, `line_height_ratio`,
`line_height(font_cache, appearance)`, `em_width`, `line_len(display_row)`, `max_point`,
`point_for_offset`.

**Positions for popups:**
- `cache_buffer_point(point, id, ctx)` saves the on-screen position of a buffer point under
  `position_id_for_cached_point(view_id, id)`.
- The cursor is always saved under `position_id_for_cursor(view_id)`, and the first cursor
  under `position_id_for_first_cursor(view_id)`.
- Anchor popovers to these ids with WarpUI's positioning.

**Clipboard text:** `clipboard_text_content(ClipboardContent) -> String`.

### 3.5 Actions (`EditorAction`)

Dispatched by keybindings and by the element. Parents can dispatch them too.

- Text input: `UserInsert`, `SetMarkedText`, `ClearMarkedText`, `ImeCommit`.
- Keys: `Enter`, `ShiftEnter`, `AltEnter`, `CtrlEnter`, `CmdEnter`, `Newline`, `Tab`,
  `ShiftTab`, `Escape`, `Backspace`, `Delete`.
- Clipboard: `Copy`, `Cut`, `Paste`, `MiddleClickPaste`, `Yank`.
- Deletion: `CutWordLeft/Right`, `DeleteWordLeft/Right`, `CutAllLeft/Right`,
  `DeleteAllLeft/Right`, `ClearAndCopyLines`.
- Movement and selection: the move, select, page and `CmdUp`/`CmdDown` actions,
  `Select(SelectAction)`, `SelectWord`, `SelectLine`, `SelectAll`, `AddNextOccurrence`,
  `AddCursorAbove`, `AddCursorBelow`.
- Other: `Scroll`, `Undo`, `Redo`, `Focus`, `ClearParentSelections`, `ShowCharacterPalette`.

### 3.6 Keybindings

All bindings require the `EditorView` keymap context and no open IME composition.
`cmdorctrl` is Cmd on macOS and Ctrl elsewhere; `meta` is Alt.

| Keys | Action | Platforms |
|---|---|---|
| Enter, numpad Enter / Shift+Enter / Alt+Enter / Ctrl+Enter | `Enter` / `ShiftEnter` / `AltEnter` / `CtrlEnter` | all |
| Cmd+Enter / Ctrl+Shift+Enter | `CmdEnter` | macOS / others |
| Esc, Backspace, Shift+Backspace, Delete, Tab, Shift+Tab | as named | all |
| cmdorctrl+C / X / V | Copy / Cut / Paste | all |
| cmdorctrl+Z, cmdorctrl+Shift+Z | Undo, Redo | all |
| cmdorctrl+A | Select all | all |
| Arrows, Shift+arrows, Shift+Home/End, PageUp/PageDown | move / select | all |
| Alt+←/→ / Ctrl+←/→ (also Meta+F/B, Meta+←/→) | word left/right | macOS / others |
| Shift+Alt+←/→ / Shift+Ctrl+←/→ | select word left/right | macOS / others |
| Alt+Backspace / Ctrl+Backspace, Alt+Delete / Ctrl+Delete | delete word left/right | macOS / others |
| Cmd+←/→ / Home, End | visual line start/end | macOS / others |
| Home, End | buffer start/end | macOS |
| Cmd+↑/↓ / Ctrl+Home/End (with Shift: select) | buffer start/end | macOS / others |
| Ctrl+Shift+↑/↓ | add cursor above/below | all |
| Meta+A/E, Meta+Shift+{ / } | paragraph start/end | all |
| Meta+Shift+< / > | buffer start/end | all |
| Meta+Backspace, Meta+D | cut word left/right | all |
| Ctrl+A/E/B/F/P/N/H/D/K/U/W/J/Y/G, Alt+D, Ctrl+Alt+arrows | emacs editing, subword movement, next occurrence | macOS only |
| Cmd+Backspace / Cmd+Delete, Cmd+Shift+K | delete all left/right, clear lines | macOS only |
| Ctrl+Cmd+Space | character palette | macOS only |

Bindings registered as editable (`editor_view:*`, `editor:*`) can be rebound once MatterLite
has a keybinding editor. Extra keymap context flag: `EditorView_SingleCursorBufferEnd`.

### 3.7 Settings

| Group | Field | `settings.toml` path | Default |
|---|---|---|---|
| `EditorSettings` | `cursor_blink` | `appearance.cursor.cursor_blink` | `enabled` |
| | `cursor_display_type` | `appearance.cursor.cursor_display_type` | `bar` (also `block`, `underline`) |
| | `autocomplete_symbols` | `text_editing.autocomplete_symbols` | `true` |
| `SelectionSettings` | `linux_selection_clipboard` | `system.linux_selection_clipboard` | `true` (Linux only) |
| | `middle_click_paste_enabled` | `text_editing.middle_click_paste_enabled` | `true` (Windows, macOS) |

The user's cursor style applies only to editors created with `allow_user_cursor_preference`.

### 3.8 Data types

`Point`, `DisplayPoint`, `EditOrigin` (`UserTyped`, `UserInitiated`, `SystemEdit`),
`InteractionState`, `SelectAction`, `EditorSnapshot`, `TextRun`, `TextStyleOperation`, `Chars`,
`LocalDrawableSelectionData`, `EditorElement`, `CursorBlink`, `CursorDisplayType`,
`PlainTextEditorViewAction`, `SelectionInsertion`, `ValidInputType`, `BaselinePositionComputationMethod`,
`PropagateAndNoOpNavigationKeys`, `PropagateHorizontalNavigationKeys`, and the constants
`SELECT_UP_ACTION_NAME`, `SELECT_DOWN_ACTION_NAME`.

---

## 4. Where to use it in MatterLite

Section numbers refer to `docs/MatterLite.md`.

### Login view (§10.2 `LoginView`)

- **Server URL, username:** `single_line`, `select_all_on_focus`, a placeholder ("https://chat.example.com").
  `Event::Enter` moves to the next step; `Event::Edited` clears an error message.
- **Password:** `is_password: true` (dots, no copy or cut).
- **MFA code:** `valid_input_type: PositiveInteger` and `max_buffer_len: Some(6)`.
- **While a request runs:** `set_interaction_state(InteractionState::Disabled)` greys out and locks the fields.

### Search bars (§10.10; History, Files and Search panes; chat filter)

- **Field:** `single_line` with the UI font (`TextOptions::ui_text`) and a placeholder.
  - "search messages…" via `set_placeholder_text`.
  - Per-prefix hints via `set_placeholder_text_with_prefix`, e.g. "from:" → "username".
- **Results:** `Event::Edited` re-runs the search; `Event::Enter` jumps to the first result.
- **Clear and close:** `Event::Escape` clears or closes; the Clear (x) button calls `clear_buffer`.
- **Into the result list:** `propagate_and_no_op_vertical_navigation_keys: Always`, so Up/Down
  arrive as `Event::Navigate` and move through the list while the field keeps focus.
- **Ctrl/Cmd+F:** focusing the bar with `select_all_on_focus` lets the user retype the query at once.

### Chat switcher (Ctrl/Cmd+K, §10.2 `ChannelSwitcher`)

- **List navigation:** a single-line field with `Navigate` (`Always`) for the result list.
- **Opening a chat:** `Enter` / `CmdEnter` opens it in the current or a new tab.
- **Closing:** `Escape` closes the switcher.

### Composer (§13) and the thread reply composer (right panel)

- **Basic setup:** `EditorView::new` with `autogrow`, `soft_wrap` and the UI font.
  `max_buffer_len` from the server's `MaxPostSize`.
- **Send key:** `EnterSettings` covers the send preference.
  - Default: Enter emits, so the parent sends, and Shift+Enter inserts a newline.
  - With `send_on_ctrl_enter`: set `enter` to `InsertNewLineIfMultiLine` and send on `CtrlEnter` / `CmdEnter`.
- **↑ edits the last post:** `propagate_and_no_op_vertical_navigation_keys: AtBoundary` plus
  `is_empty`. Alternatively, `Navigate(Up)` arrives when the cursor is on the first row.
- **Autocomplete popovers (`@`, `~`, `:`, `/`):**
  - Detect the trigger with `chars_preceding_selections` or `buffer_text` and the cursor offset.
  - Anchor the popover at `position_id_for_cursor(view_id)`, or at a point saved with `cache_buffer_point`.
  - While the popover is open, set a keymap flag with `keymap_context_modifier` so Up/Down/Enter/Tab go to the popover.
  - Insert the choice with `select_and_replace` or `replace_last_n_characters`; it's undoable.
- **Typing indicator:** `Event::Edited(origin)` with `origin.is_user()` triggers the throttled `user_typing` message.
- **Drafts:** save on `Edited` (debounced). Restore on channel switch with `system_reset_buffer_text` (clears undo, so restoring isn't undoable). Use `set_base_buffer_text` / `is_dirty` to know whether anything changed.
- **Pasting files and images:** `delegate_paste_handling: true` lets the parent read the clipboard and upload files, and insert text itself. Drag-and-drop already reaches the parent.
- **Highlighting in the text:** `update_buffer_styles` with `TextStyleOperation` colours `@mentions`, `~channels`, `:emoji:` and Markdown syntax, or underlines errors, without changing the text.
- **Above the text:** `render_decorator_elements` → `top_section` shows a strip such as "Editing message — Esc to cancel" or "Replying to …".
- **Disabled state:** `set_interaction_state(Disabled)` when the channel is read-only or archived.
- **Vim mode:** removed in this extraction. MatterLite.md §13 lists it as optional; adding it back would mean porting Warp's `VimHandler` implementation and the vim state model.

### Editing a sent message

Same as the composer, inline in the timeline: `set_buffer_text_with_base_buffer(original)`,
`is_dirty` to enable Save, `Escape` to cancel, `Enter` to save.

### Dialogs and settings (§10.2 `SettingsView`, channel actions)

- **Create channel:**
  - Name: `single_line`.
  - URL slug: `valid_input_type: NoSpaces` with a max length.
  - Purpose: a small multi-line editor with `autogrow`.
- **Channel header and purpose editing** (Channel tab → Info): multi-line with soft wrap.
- **Numeric settings** (font size, tab limit, LRU cap): `PositiveInteger`, `select_all_on_focus`, `clear_selections_on_blur`.
- **Rename tab** (local alias, §10.7): `single_line`, `select_all_on_focus`, `Enter` to apply, `Escape` to cancel.

### Read-only, copyable text

`InteractionState::Selectable` turns an editor into selectable, copyable text with no editing.
Uses: the "Copy link" field, post edit history in the right panel, and server or error details
in the connection banner.

### Monospace

`baseline_position_computation_method: Grid` with the monospace font gives grid-aligned text for
code-like inputs, such as a code-block editor or slash-command arguments.

---

## 5. Not included

Removed during the extraction (see the commit `adapt(ml_editor): extract Warp's EditorView into
EditorView.rs`):

- vim mode (what it did and how to bring it back: [`MatterLite-ComposerVim.md`](MatterLite-ComposerVim.md))
- voice input
- the AI "@" context menu
- image and file attachments for AI
- AI input mode
- autosuggestions and their hint chips
- command x-ray
- smart double-click selection
- fold and unfold commands
- shared-session peers: remote cursors and avatars, remote edits, display-only buffers
  (needed for collaborative drafts; see [`MatterLite-Collaborate.md`](MatterLite-Collaborate.md) §8
  for each removed item and where it is in Warp)
- terminal-only behaviour: ctrl-c clearing, alt-. last word, alt-space non-expanding space,
  shell path escaping, drag-and-drop path insertion, telemetry for unknown shortcuts

Warp's own editor tests (about 18,000 lines across `view/mod_tests.rs`, `model/mod_tests.rs`,
`buffer/mod_tests.rs` and others) depend on Warp's app crate and were not ported.
