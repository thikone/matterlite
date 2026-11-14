# File viewers: M0 spike

The read-only spike of `ml_file_views` asked for in [MatterLite.md](MatterLite.md) §14.4 step 5:
open a local `.rs` and `.md` file in a WarpUI window to size the viewer work before M5.

```text
cargo run -p ml_file_views --example file_viewer -- README.md crates/ml_file_views/src/view.rs
```

Without arguments the example opens the repository README and `ml_file_views/src/view.rs`.
Ctrl+1…9 switch files; `MATTERLITE_THEME=dracula` (any theme name in snake_case) changes the
theme.

## Result

**Don't extract Warp's viewers; build thin views on `warp_editor`.** §14.4 planned to copy
Warp's `app/src/code/` and `app/src/notebooks/` (about 31,000 lines) and cut them down. The
spike shows the work is in the `warp_editor` crate, which MatterLite already has: buffer,
Markdown/Jupyter parsing, layout, rich-text rendering (headings, lists, tables, code blocks,
Mermaid, images, task lists), selection and scrolling. Warp's app-level code wraps that in
editing, diffs, review comments, LSP, Vim and notebook commands. A read-only viewer needs none of
that. A new view over `warp_editor` took about 1,300 lines; only the text styles and the file
routing are adapted from Warp's app code.

| File | Lines | Contents |
|---|---:|---|
| `src/model.rs` | 376 | `FileViewModel`: buffer, selection and render state; loads Markdown, notebooks or plain text; syntax highlighting with `syntax_tree` in the theme's ANSI colours; copy |
| `src/view.rs` | 667 | `FileView`: header (name, kind, line count, Raw toggle), `RichTextElement` in a vertical `Scrollable`, mouse selection, keys, `EditorView` and `RichTextAction` implementations |
| `src/styles.rs` | 153 | text styles for documents and code, adapted from Warp |
| `src/file_type.rs` | 88 | routing to Markdown, notebook, code, text, image or external, adapted from Warp |
| `examples/file_viewer.rs` | 218 | the spike window |
| tests | 293 | 12 tests: routing, loading each format, languages, copy, Raw toggle, placeholders |

## What works (checked on Windows with screenshots)

- **Markdown**: headings, bold, links, inline code, fenced code blocks, lists, tables. A **Show
  Raw / Show Rendered** button switches to the source and back.
- **Jupyter notebooks**: Markdown cells, code cells, their text output and Markdown tables.
- **Code**: highlighted by `languages` + `syntax_tree` (Rust, TOML, …) in the theme's colours. A
  15,507-line file opens and pages smoothly in a debug build.
- **Plain text** (`.log`, unknown text extensions): monospace lines.
- **Routing**: images and binary, unreadable or oversized files (> 20 MB) get a placeholder
  instead of a viewer; unknown extensions are judged by their first kilobyte.
- **Reading**: ↑/↓ and PageUp/PageDown scroll. Dragging selects text and double-clicking
  selects a word (both checked with a real mouse and Ctrl+C); Ctrl/Cmd+A selects everything.
  Copying a rendered document also puts HTML on the clipboard. Mouse-wheel scrolling and
  triple-click (line) selection are wired the same way but weren't exercised.
- **Themes**: styles and highlight colours follow `Appearance`, including theme changes.

## What's missing, with sources to take it from

| Missing | Notes | Warp source | Size there |
|---|---|---|---:|
| Horizontal scrolling for code | Long lines are cut at the window edge | `code/editor/view.rs` (`ScrollHorizontal`), `code/editor/scroll.rs` | ~150 |
| Line numbers | Gutter is reserved but empty | `code/editor/element.rs` (gutter painting; drop the diff and comment buttons) | 1,694 → ~400 |
| Highlighting inside document code blocks | Blocks render, but monochrome | notebook child models (`NotebookCommand`, cached highlighting) in `notebooks/editor/` | ~300 needed |
| Find in file | | `code/editor/find/view.rs`, `notebooks/editor/find_bar.rs`; `warp_editor::search` already does the matching | 1,047 / 631 |
| Go to line | | `code/editor/goto_line/view.rs` | 201 |
| Clicking links | Links render; clicks aren't handled (needs `LinkHandler`, §14.4) | `notebooks/link.rs`, `RichTextAction::left_mouse_up` link handling | ~500 |
| Image lightbox | | `crates/ui_components/src/lightbox.rs` (already imported), `workspace/lightbox_view.rs` | 338 + 212 |
| Editing and saving | Dirty state, Save as…, "send as new version" (§14.2) | `code/local_code_editor.rs` + `warp_files::FileModel` | 2,746 (most is LSP and diffs) |
| Vim mode | Shared with the composer ([MatterLite-ComposerVim.md](MatterLite-ComposerVim.md)) | `code/editor/view/vim_handler.rs` | 1,037 |
| External-app fallback | "Open with system app" (with a confirmation for executables) and "Save as…" | new | small |

Rough total for the M5 viewer set (read-only features above, without editing or Vim): about
3,000–4,000 lines, much of it adapted from the Warp files listed. Editing adds an
`EditorModel`-style edit path on the same model (`warp_editor` already implements the edits)
plus the save flows.

## Notes for M5

- `DisplayOptions::default()` has `debug_bounds: true`; every view must set it to `false`.
- `RenderState::scroll` uses wheel deltas: positive scrolls up.
- Selection changes must be pushed to the render state (`update_selection`, with render offsets
  one less than buffer offsets), as Warp's models do; nothing does it automatically.
- Markdown needs `set_document_path` so relative image links resolve.
- The viewer reads the whole file synchronously. Fine for cached attachments under the 20 MB
  limit, but loading should move off the UI thread with `warp_files::FileModel` in M5.
