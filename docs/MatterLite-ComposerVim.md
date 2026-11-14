# MatterLite — Vim mode in the composer

Warp had a Vim mode for its text inputs. It was removed from `ml_editor` during the
extraction (see [`EditorView.md`](EditorView.md) §5). This document records what it was, how
it was built, how it would fit MatterLite, the design decisions it needs, and where to get the
code back from Warp.

**Recommendation:**
- **Timing:** not in the MVP. Add it around M5, together with `ml_file_views`, because Warp's
  code editor brings its own Vim support.
- **Settings:** one opt-in setting, off by default, and one set of registers, shared by the
  composer and the code editor.
- **Scope:** only multi-line editors that opt in, never single-line fields.

Related: [`MatterLite.md`](MatterLite.md) §13 (composer), §14 (file viewers),
[`MatterLite-Collaborate.md`](MatterLite-Collaborate.md) (collaborative drafts).

Warp paths below are relative to `C:\Projects\Libs\warp-terminal` @ `5bb7bcc6a`; line numbers
are in Warp's files. The editor files are also in MatterLite's import commit `79a4f9f`
(`git show 79a4f9f:crates/ml_editor/src/warp_import/editor/<path>`).

---

## Contents

1. [What it was in Warp](#1-what-it-was-in-warp)
2. [Features](#2-features)
3. [How it was built](#3-how-it-was-built)
4. [How it fits MatterLite](#4-how-it-fits-matterlite)
5. [Composer design](#5-composer-design)
6. [Settings](#6-settings)
7. [Bringing it back](#7-bringing-it-back)
8. [Open questions](#8-open-questions)

---

## 1. What it was in Warp

Warp replaces the shell's own line editor (readline, zle), so users who edit commands in vi
style (`set -o vi` in bash, `bindkey -v` in zsh) needed the same in Warp's command input.
Vim mode served them, and was later reused by Warp's code editor.

**When it was active:**
- **Two switches:** it needed both the user setting `vim_mode` (off by default) and the editor
  opting in with `EditorOptions::supports_vim_mode`.
- **Warp's editors:** only the terminal command input opted in. Warp's code editor
  (`app/src/code/editor`) has its own Vim support built on the same crate.
- **Single-line fields** (search boxes, rename fields, the font-size field) never opted in;
  `SingleLineEditorOptions` always set `supports_vim_mode: false`.

## 2. Features

| Area | What it did |
|---|---|
| Modes | Normal, Insert, Visual (charwise and linewise), Replace |
| Cursor | Block in Normal and Visual, underline in Replace, bar in Insert |
| Motions | `h j k l`; words `w b e` and `W B E`; `0 ^ $`; find a character on the line `f t F T`, repeated with `; ,`; paragraphs `{ }`; matching bracket `%`; unmatched bracket `[( ])` style jumps; first, last or numbered line `gg G`. Counts work, e.g. `3w`, `2dd` |
| Operators | delete, change, yank (`d c y`) with a motion or a text object; linewise forms (`dd`, `cc`, `yy`); case changes |
| Text objects | inner and around word, WORD, quotes, brackets and paragraphs (`iw aw iW aW i" a" i( a( ip ap`, …) |
| Other edits | replace one character (`r`), Replace mode (`R`), toggle case (`~`), join lines (`J`), paste after and before (`p P`), undo (`u`), redo (Ctrl+R in Normal mode) |
| Repeat | `.` repeats the last change |
| Registers | yanked and deleted text goes into Vim registers; optionally the unnamed register is the system clipboard |
| Visual mode | own selection range (from the "visual tail" to the cursor), drawn separately from normal selections; operators and paste on the selection |
| `K` | "keyword program": asked the parent to look up the word under the cursor |
| Handed to the parent | search (`/ ? * #` → `Event::Search { direction, term }`), command line (`:` → `Event::ExCommand`), mode changes (`Event::VimStatusUpdate`, for a mode indicator), and Up/Down at the first or last line in Normal mode (`Event::Navigate`) |
| Esc handling | `PropagateAndNoOpEscapeKey`: either the editor leaves Insert mode first (`HandleFirst`), or the parent handles Esc first (`PropagateFirst`, e.g. to close completions) |
| Line capping | in Normal mode the block cursor can't go past the last character of a line, as in Vim |
| IME | text from an input method was ignored in Normal and Visual mode |

**In Warp's terminal input** (`app/src/terminal/input.rs`, ~L11103–11116):
- `Event::Search` opened the terminal's find bar.
- `Event::ExCommand` opened the command search panel (`ShowCommandSearch`).
- `Event::VimStatusUpdate` refreshed the mode shown in the input (the `vim_status_bar` setting).

## 3. How it was built

The code is in three layers. Only the first is in MatterLite today.

| Layer | Where | Size | In MatterLite |
|---|---|---:|---|
| **Vim state machine** (crate `vim`): turns keystrokes into commands ("delete inside word ×2"), tracks mode, pending operator, counts, last find, dot-repeat; word and paragraph iterators; `VimHandler` and `VimSubscriber` traits | `crates/vim/src/vim.rs` and iterators | 2,243 + 325 (+1,326 tests) | **Yes**, `crates/vim` (verbatim) |
| **Executing the commands on the editor**: `impl VimHandler for EditorView` plus the model operations it calls | `app/src/editor/view/mod.rs`, `view/model/mod.rs`, `view/element.rs`, `view/snapshot.rs` | ~2,200 | **No**, removed |
| **Registers** (`VimRegisters` singleton: named and unnamed registers, optional system clipboard) | `app/src/vim_registers.rs` | 103 | **No**, never imported |

Warp's code editor has a separate `impl VimHandler for CodeEditorView` in
`app/src/code/editor/view/vim_handler.rs` (1,037 lines). It arrives with `ml_file_views`
(MatterLite.md §14.4) and uses the same `vim` crate and registers.

### 3.1 Editor view (`app/src/editor/view/mod.rs`)

| Item | Location | Purpose |
|---|---|---|
| `impl VimHandler for EditorView` | L1979–2927 (948 lines) | Every Vim command: insert, motions, find, paragraphs, operators with motions and text objects, replace, case, search, ex command, jumps, paste, join, undo, visual operators, visual paste, visual text objects, backspace, delete, escape, mode changes |
| Vim model creation and subscription | L3088–3090 (`VimModel::new`, `handle_vim_event` from the `VimSubscriber` trait) | One state machine per editor |
| `supports_vim_mode` option and field; `PropagateAndNoOpEscapeKey` and `propagate_and_no_op_escape_key` | L1453, L1828–1830; L1337, L1445, L1789 | Opt-in per editor; Esc precedence |
| Vim helpers | L5015 `vim_mode_enabled`; L5446–5495 `vim_user_insert`, `vim_keystroke`, `vim_force_insert_mode`, `vim_interrupt`; L7673 `vim_maybe_enforce_cursor_line_cap`; L7777–7950 `vim_mode`, `vim_state`, `vim_has_pending`, `vim_escape`, `vim_apply_insert_position`, `vim_set_visual_tail`, `vim_cursor_forward_word`, `vim_cursor_backward_word` (~250 lines) | Routing typed text, forcing Insert mode on mouse selection or submit, line capping |
| Navigation hand-off in Normal mode | L6631–6655 `vim_should_propagate_upward/downward_navigation`, used from the handler at ~L2070 | `k`/`j` at the edge go to the parent |
| Esc | L6183–6200 in `escape` | Vim escape first when something is pending |
| Line capping in queries | L6725 in `single_cursor_at_buffer_end` (`respect_line_cap`) | |
| IME guards | ~L5459 (`ime_commit`), ~L8248 (`set_marked_text`) | No IME text in Normal and Visual |
| Calls to `vim_force_insert_mode` | in mouse selection, keyboard selection, word and line selection, `select_all` and submit | Selecting text with the mouse or shift-keys switches to Insert |
| Actions and dispatch | L999 `VimUserInsert`, L1029 `VimBackspace`, L1031 `VimEscape`; L8695–8700 in `handle_action` | |
| Keymap context | L8829–8831: flags `Vim`, `VimNormalMode` | |
| Bindings | L200–205 (Backspace goes to Vim when `Vim` is set), L803 (clear lines not in Vim), L980–990 (Ctrl+[ exits Insert; Ctrl+R redo in Normal mode) | |
| Events | `Search`, `ExCommand`, `VimStatusUpdate` in `pub enum Event` (L8458–8471) | |

### 3.2 Editor model (`app/src/editor/view/model/mod.rs`, ~860 lines)

| Item | Location | Purpose |
|---|---|---|
| `vim_visual_tails` field and its upkeep | L432, L563, L860, L1330 (`vim_visual_tail_offsets`), L1425 (restored with snapshots), L2958–2967 | Where Visual mode started, per cursor |
| Visual selection | L1567 `extend_selection_linewise`, L1581 `extend_selection_linewise_visual_mode`, L1679 `vim_visual_selection_range` | |
| Movement | L1613, L1658 bracket jumps; L1736 `move_cursors_by_offset`; L1797 `move_cursor_ignoring_newlines`; L1875, L1916 `move_up/down_by_offset`; L1946 `vim_find_char`; L2625 `vim_move_by_paragraph` | |
| Selections for operators | L2363 `vim_select_words`, L2440 `…_for_char_motion`, L2498 `…_for_line_motion`, L2521 `…_for_first_nonwhitespace_motion`, L2547 `…_for_matching_bracket`, L2575 `vim_select_text_object` | |
| Case | L1979 `toggle_selection_case`, L2006, L2020 `selection_to_uppercase/lowercase` | |
| Line capping | L2664 `vim_needs_line_capping`, L2677 `vim_enforce_cursor_line_cap` | |
| Registers | L2846 `copy_selection_to_vim_register` (uses `VimRegisters`) | |

### 3.3 Rendering (`app/src/editor/view/element.rs`, `snapshot.rs`, ~120 lines)

| Item | Location | Purpose |
|---|---|---|
| `vim_mode` on `EditorElement` | `element.rs` L203, L224 | |
| Cursor shape per mode (`get_cursor_type`) | `element.rs` ~L1530–1540 | Block, underline, bar |
| Visual-mode selection drawing | `element.rs` L943, L1128–1200 (charwise and linewise) | |
| Typed text routed to `VimUserInsert` | `element.rs` L618 | |
| `vim_visual_tails` | `snapshot.rs` L627 | |

### 3.4 Settings and tests

- **Settings:** in `app/src/settings/editor.rs` (`AppEditorSettings`): `vim_mode` (L204,
  `text_editing.vim_mode_enabled`), `vim_unnamed_system_clipboard` (L214), `vim_status_bar`
  (L224). The settings page UI is in `app/src/settings_view/features_page.rs`.
- **Tests:** `app/src/editor/view/vim_handler_tests.rs` (7,864 lines) covers the editor
  integration. The `vim` crate's own tests are already in MatterLite.

## 4. How it fits MatterLite

**Good fits:**

- **Code editor in the file viewers** (`ml_file_views`, M5). Editing a config file or a
  code attachment is where Vim users expect Vim most, and Warp's code editor already
  supports it (MatterLite.md §14.1).
- **Multi-line composers, as an option:** channel composer, thread composer, inline message
  editing. Developer-heavy teams write long messages with code blocks, and the Mattermost
  webapp has no Vim mode, so it sets MatterLite apart for keyboard-heavy users.

**Poor fits:**

- **Single-line inputs:** search bars, login, dialogs, the chat switcher. Never enable it there.
- **Most messages are short.** For a one-line reply, modal editing gains little, which is why
  it stays opt-in and off by default.

## 5. Composer design

The composer uses keys that Vim also wants. Proposed rules:

| Topic | Proposal | Why |
|---|---|---|
| Starting mode | Always **Insert** when the composer gets focus and after sending | Nobody should land in Normal mode in a chat box. Warp also forced Insert after running a command |
| Esc | Insert or Visual mode → Normal mode only. Normal mode with nothing pending → the app's Esc action (close the autocomplete popover, cancel the message edit, close the right panel, mark the chat read) | Restore `PropagateAndNoOpEscapeKey::HandleFirst` for the composer. An open autocomplete popover takes Esc before Vim does |
| Enter | Sends in Insert mode. In Normal mode, a setting: send (Warp's behaviour) or do nothing | Avoids accidental sends while navigating |
| Shift+Enter, Ctrl/Cmd+Enter | Unchanged, per `EnterSettings` | |
| Autocomplete (`@ ~ : /`) | Insert mode only | In Normal mode these keys are Vim commands |
| `/`, `?`, `*`, `#` | Open the History pane's search bar for the current chat (MatterLite.md §10.8), prefilled with the term for `*` and `#` | Same meaning as in Vim: search the conversation |
| `:` | Open the command palette, or a slash-command entry | Warp opened its command search here |
| `K` | Look up the word under the cursor in server search, or do nothing | |
| ↑ edits the last post | Up arrow in an empty composer, in either mode; `k` in Normal mode on an empty composer does the same | Keep the familiar behaviour |
| Ctrl+[ | Leaves Insert mode | As in Warp |
| Mode indicator | A small NORMAL / VISUAL / REPLACE badge in the composer, drawn with `EditorOptions::render_decorator_elements` and updated on `Event::VimStatusUpdate`; hidden in Insert mode | Warp's `vim_status_bar` |
| Mouse and shift-selection | Switch to Insert mode, as Warp did | |
| Clipboard | Unnamed register = system clipboard by default in MatterLite | Chat users expect `y` then Ctrl+V elsewhere to work; still a setting |
| IME | Ignored in Normal and Visual mode | As in Warp |
| Emacs bindings (macOS) | Unchanged in Insert mode; in Normal mode Vim keys take precedence through the `Vim` / `VimNormalMode` keymap flags | |
| Collaborative drafts | Compatible: Vim commands produce ordinary edits on the shared buffer; visual-mode state and registers stay local | [`MatterLite-Collaborate.md`](MatterLite-Collaborate.md) |

## 6. Settings

One group shared by the composer and the code editor. Names follow Warp's `settings.toml`
paths.

| Setting | Path | Default |
|---|---|---|
| Vim mode | `text_editing.vim_mode_enabled` | `false` |
| Unnamed register uses the system clipboard | `text_editing.vim_unnamed_system_clipboard` | `true` (Warp: `false`) |
| Show the mode badge | `text_editing.vim_status_bar` | `true` |
| Enter in Normal mode | `text_editing.vim_enter_in_normal_mode` (`send` / `ignore`) | `ignore` (new) |

As with every toggle, add a Command Palette entry to turn Vim mode on and off.

## 7. Bringing it back

1. **Import** `app/src/vim_registers.rs` and Warp's `vim_handler_tests.rs` verbatim
   (`import(warp): …`). The editor parts are already in commit `79a4f9f`.
2. **Port into `ml_editor`** (`port(warp): vim mode for EditorView`):
   - the items in §3.1–3.3;
   - the three settings into `EditorSettings`;
   - the Vim bindings and the `Vim` / `VimNormalMode` keymap flags;
   - `PropagateAndNoOpEscapeKey`.

   About 2,300 lines in `EditorView.rs`; the registers go next to `EditorSettings`.
   Add the `vim` crate to `crates/ml_editor/Cargo.toml`.
3. **Adapt** to MatterLite's rules in §5: start in Insert mode, Enter in Normal mode, the
   clipboard default. Map `Event::Search` and `Event::ExCommand` in the composer. Add the
   mode badge.
4. **Tests:** port the parts of `vim_handler_tests.rs` that don't depend on Warp's terminal or
   app test setup into `EditorView_vim_tests.rs`.
5. **Code editor:** when `ml_file_views` arrives, wire its `vim_handler.rs` to the same
   settings and registers.
6. **Docs:** update the `ml_editor` PROVENANCE row and [`EditorView.md`](EditorView.md).

## 8. Open questions

- **Separate settings?** Should Vim mode be one setting for the composer and the code editor,
  or two? One is simpler and matches Warp; two lets people use Vim for files but not for chat.
- **Thread composer:** should it follow the same setting, or stay plain because replies are
  short?
- **Mode badge position:** inside the composer (decorator) or in the composer's footer next
  to the Send button?
