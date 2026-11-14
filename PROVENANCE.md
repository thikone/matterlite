# Provenance of Warp-derived code

Reference: `C:\Projects\Libs\warp-terminal` @ `5bb7bcc6a` (2026-11-06)

One row per imported unit: a whole crate (Tier A) or a file/module (Tier B and C).
Status: `verbatim` (unchanged since import), `adapted` (modified), `rewritten` (only the ideas
remain), `snippet` (a few functions copied into new code). See AGENTS.md for the procedure.

| MatterLite path | Warp source | Commit | Status | License | Notes |
|---|---|---|---|---|---|
| crates/warp_errors/ | crates/warp_errors/ | 5bb7bcc6a | adapted | AGPL-3.0 | removed Sentry reporting and websocket classification; errors are logged only |
| crates/string-offset/ | crates/string-offset/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/sum_tree/ | crates/sum_tree/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/markdown_parser/ | crates/markdown_parser/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/settings_value/ | crates/settings_value/ | 5bb7bcc6a | adapted | AGPL-3.0 | derive tests enable the `derive` feature through a dev-dependency |
| crates/settings_value_derive/ | crates/settings_value_derive/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/command/ | crates/command/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/warp_util/ | crates/warp_util/ | 5bb7bcc6a | adapted | AGPL-3.0 | Tier C: kept content_version, file, file_type, path, standardized_path, user_input, windows; removed git, worktree, remote-host, sync and on_cancel helpers |
| crates/warpui_core/ | crates/warpui_core/ | 5bb7bcc6a | adapted | MIT | removed the TUI front-end (`tui` feature, elements/tui, runtime), wasm deps; Roboto path points at assets/; test GIF moved to test_data/ |
| crates/warpui/ | crates/warpui/ | 5bb7bcc6a | adapted | MIT | removed the wasm platform and wasm deps; only the list and scrolling examples; font tests read assets/bundled/fonts |
| crates/virtual_fs/ | crates/virtual_fs/ | 5bb7bcc6a | adapted | AGPL-3.0 | test helper for warpui; one clippy fix |
| crates/asset_cache/ | crates/asset_cache/ | 5bb7bcc6a | adapted | AGPL-3.0 | wasm branches removed |
| crates/asset_macro/ | crates/asset_macro/ | 5bb7bcc6a | adapted | AGPL-3.0 | assets resolved from the repository assets/ dir; remote_asset! removed; bundled_or_fetched_asset! always bundles; warp_util::assets constants inlined |
| crates/warpui_extras/ | crates/warpui_extras/ | 5bb7bcc6a | adapted | AGPL-3.0 | removed registry, UserDefaults and localStorage preference stores; TOML and JSON-file stores always built; wasm branch removed |
| crates/warp_features/ | crates/warp_features/ | 5bb7bcc6a | verbatim | AGPL-3.0 | Warp flag list still complete; pruned to MatterLite flags at the end of M0 |
| crates/settings/ | crates/settings/ | 5bb7bcc6a | adapted | AGPL-3.0 | no integration_tests feature (new_with_defaults is test-only) |
| crates/warp_core/ | crates/warp_core/ | 5bb7bcc6a | adapted | AGPL-3.0 | Tier B: kept app_id, assertions, async, features, platform, safe_log, ui (appearance, theme, icons, color, builder), user_preferences; removed channels, context flags, telemetry, execution mode, sync queue, semantic selection and the other Warp product modules; safe_log full output in debug builds |
| crates/warp_core/src/paths.rs | crates/warp_core/src/paths.rs | 5bb7bcc6a | rewritten | AGPL-3.0 | MatterLite storage layout (§9); portable data/ detection kept |
| crates/ui_components/ | crates/ui_components/ | 5bb7bcc6a | adapted | AGPL-3.0 | library example embeds the repository assets/ |
| crates/vim/ | crates/vim/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/ipynb_parser/ | crates/ipynb_parser/ | 5bb7bcc6a | verbatim | AGPL-3.0 | needed by warp_editor (notebook rendering) |
| crates/warp_editor/ | crates/editor/ | 5bb7bcc6a | adapted | AGPL-3.0 | debug check uses cfg!(debug_assertions) instead of Warp channels; clippy 1.98 fixes |
| crates/languages/ | crates/languages/ | 5bb7bcc6a | adapted | AGPL-3.0 | warp_editor dependency through the workspace |
| crates/syntax_tree/ | crates/syntax_tree/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/watcher/ | crates/watcher/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/warp_files/ | crates/warp_files/ | 5bb7bcc6a | adapted | AGPL-3.0 | local files only: removed the remote-host backend (remote_server) and git repository subscriptions (repo_metadata); paths canonicalized with dunce |
| crates/fuzzy_match/ | crates/fuzzy_match/ | 5bb7bcc6a | verbatim | AGPL-3.0 | |
| crates/ml_themes/src/theme.rs | app/src/themes/theme.rs | 5bb7bcc6a | adapted | AGPL-3.0 | no referral or image-generated (in-memory) themes, no cloud-sync portability checks, no terminal preview/prompt colors; custom theme paths relative to warp_core::paths::themes_dir |
| crates/ml_themes/src/default_themes.rs | app/src/themes/default_themes.rs | 5bb7bcc6a | adapted | AGPL-3.0 | referral themes removed |
| crates/ml_themes/src/settings.rs | app/src/settings/theme.rs | 5bb7bcc6a | adapted | AGPL-3.0 | settings never sync to a cloud |
| crates/ml_themes/src/appearance.rs | app/src/appearance.rs | 5bb7bcc6a | rewritten | AGPL-3.0 | font loading and theme refresh only; no font settings, app icons or wasm events |
| crates/ml_themes/src/warp_import/themes/theme_chooser.rs | app/src/themes/theme_chooser.rs | 5bb7bcc6a | verbatim | AGPL-3.0 | not compiled; adapted together with ml_workspace |
| crates/ml_workspace/src/buttons.rs | app/src/ui_components/buttons.rs | 5bb7bcc6a | adapted | AGPL-3.0 | icon_button_with_color and its styles only |
| crates/ml_workspace/src/warp_import/session_history.rs | app/src/workspace/view/session_history.rs | 5bb7bcc6a | verbatim | AGPL-3.0 | not compiled; source for SearchBarView |
| crates/ml_editor/src/EditorView.rs | app/src/editor/ (view, model, soft_wrap), app/src/settings/editor.rs, app/src/settings/select.rs, app/src/util/clipboard.rs, app/src/util/extensions.rs, app/src/terminal/grid_size_util.rs | 5bb7bcc6a | adapted | AGPL-3.0 | merged into one file (submodules inline); removed vim, voice, AI context menu, image attachments, autosuggestions, command x-ray, smart select, folding actions, shared-session peers and remote CRDT ops; EditorSettings/SelectionSettings never sync; emacs-style ctrl bindings macOS-only; Warp's tests not ported |
| crates/ml_logging/src/lib.rs | crates/warp_logging/src/rotation.rs | 5bb7bcc6a | rewritten | AGPL-3.0 | startup and in-session rotation, env_logger; no Sentry, crash recovery or log bundles |
| assets/async/jpg/ | app/assets/async/jpg/ | 5bb7bcc6a | verbatim | AGPL-3.0 | background images of the built-in themes (10 files) |
| assets/bundled/fonts/password.ttf | app/assets/bundled/fonts/password.ttf | 5bb7bcc6a | verbatim | AGPL-3.0 | password dots font |
| assets/bundled/fonts/hack/ | app/assets/bundled/fonts/hack/ | 5bb7bcc6a | verbatim | MIT | Hack font with its LICENSE.md; monospace font |
| assets/bundled/svg/ | app/assets/bundled/svg/ | 5bb7bcc6a | verbatim | AGPL-3.0 | only the icons referenced by imported code (341 of 383) |
| assets/bundled/fonts/roboto/ | app/assets/bundled/fonts/roboto/ | 5bb7bcc6a | verbatim | Apache-2.0 | font files; LICENSE.txt included |

Rows with status `verbatim` differ from the reference only by the provenance header.
