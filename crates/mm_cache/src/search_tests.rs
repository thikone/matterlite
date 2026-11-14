// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::json;

use super::*;

#[test]
fn queries_are_quoted_and_the_last_word_is_a_prefix() {
    assert_eq!(fts_query("  "), None);
    assert_eq!(fts_query("rele").as_deref(), Some(r#""rele"*"#));
    assert_eq!(
        fts_query("tag the rel").as_deref(),
        Some(r#""tag" "the" "rel"*"#)
    );
    // FTS5 operators and quotes stay literal.
    assert_eq!(
        fts_query(r#"a OR "b"#).as_deref(),
        Some(r#""a" "OR" """b"*"#)
    );
}

#[test]
fn attachments_and_file_names_are_indexed() {
    let post: Post = serde_json::from_value(json!({
        "message": "",
        "props": {"attachments": [{
            "pretext": "Build", "title": "Release 1.2", "text": "All green",
            "fields": [{"title": "Branch", "value": "main"}]
        }]},
        "metadata": {"files": [{"name": "notes.md"}, {"name": "main.rs"}]}
    }))
    .unwrap();
    let (text, files) = index_text(&post);
    assert_eq!(text, "\nBuild\nRelease 1.2\nAll green\nBranch\nmain");
    assert_eq!(files, "notes.md main.rs");
}
