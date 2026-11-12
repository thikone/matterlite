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

Rows with status `verbatim` differ from the reference only by the provenance header.
