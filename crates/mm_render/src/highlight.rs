// SPDX-License-Identifier: AGPL-3.0-only
// Capture-name mapping as in warp-terminal `crates/syntax_tree/src/queries/highlight_query.rs`
// (`convert_capture_name_to_color`) and language table as in `crates/languages/src/lib.rs`
// @ 5bb7bcc6a; the code is new (headless, without the editor buffer).

//! Syntax highlighting of fenced code blocks with the tree-sitter grammars and highlight
//! queries bundled in `arborium` (the ones Warp's editor uses), without any UI dependency.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use arborium::tree_sitter::{Language, Parser, Query, QueryCursor};
use streaming_iterator::StreamingIterator;

use crate::doc::{CodeSpan, Highlight};

/// Larger code blocks are shown without highlighting.
const MAX_HIGHLIGHT_BYTES: usize = 100_000;

struct Grammar {
    language: Language,
    query: Query,
    /// Per capture index.
    classes: Vec<Option<Highlight>>,
}

/// Loaded grammars by arborium name; `None` when one failed to load.
type Grammars = HashMap<&'static str, Option<Arc<Grammar>>>;

static GRAMMARS: LazyLock<Mutex<Grammars>> = LazyLock::new(Default::default);

/// The arborium grammar name and highlight query for a code fence's language, accepting the
/// names and aliases people use in Mattermost (highlight.js names).
fn grammar_source(language: &str) -> Option<(&'static str, &'static str)> {
    Some(match language {
        "rust" | "rs" => ("rust", arborium::lang_rust::HIGHLIGHTS_QUERY),
        "go" | "golang" => ("go", arborium::lang_go::HIGHLIGHTS_QUERY),
        "yaml" | "yml" => ("yaml", arborium::lang_yaml::HIGHLIGHTS_QUERY),
        "python" | "py" | "python3" => ("python", arborium::lang_python::HIGHLIGHTS_QUERY),
        "javascript" | "js" | "jsx" | "node" => {
            ("javascript", arborium::lang_javascript::HIGHLIGHTS_QUERY)
        }
        "typescript" | "ts" => ("typescript", &arborium::lang_typescript::HIGHLIGHTS_QUERY),
        "tsx" => ("tsx", &arborium::lang_tsx::HIGHLIGHTS_QUERY),
        "java" => ("java", arborium::lang_java::HIGHLIGHTS_QUERY),
        "cpp" | "c++" | "cc" | "hpp" => ("cpp", &arborium::lang_cpp::HIGHLIGHTS_QUERY),
        "c" | "h" => ("c", arborium::lang_c::HIGHLIGHTS_QUERY),
        "bash" | "sh" | "shell" | "zsh" | "console" => {
            ("bash", arborium::lang_bash::HIGHLIGHTS_QUERY)
        }
        "csharp" | "cs" | "c#" => ("c-sharp", arborium::lang_c_sharp::HIGHLIGHTS_QUERY),
        "html" | "htm" => ("html", arborium::lang_html::HIGHLIGHTS_QUERY),
        "xml" | "svg" => ("xml", arborium::lang_xml::HIGHLIGHTS_QUERY),
        "css" => ("css", arborium::lang_css::HIGHLIGHTS_QUERY),
        "json" => ("json", arborium::lang_json::HIGHLIGHTS_QUERY),
        "jq" => ("jq", arborium::lang_jq::HIGHLIGHTS_QUERY),
        "hcl" | "terraform" | "tf" => ("hcl", arborium::lang_hcl::HIGHLIGHTS_QUERY),
        "lua" => ("lua", arborium::lang_lua::HIGHLIGHTS_QUERY),
        "nix" => ("nix", arborium::lang_nix::HIGHLIGHTS_QUERY),
        "ruby" | "rb" => ("ruby", arborium::lang_ruby::HIGHLIGHTS_QUERY),
        "php" => ("php", arborium::lang_php::HIGHLIGHTS_QUERY),
        "toml" => ("toml", arborium::lang_toml::HIGHLIGHTS_QUERY),
        "swift" => ("swift", arborium::lang_swift::HIGHLIGHTS_QUERY),
        "kotlin" | "kt" => ("kotlin", arborium::lang_kotlin::HIGHLIGHTS_QUERY),
        "scala" => ("scala", arborium::lang_scala::HIGHLIGHTS_QUERY),
        "powershell" | "ps1" | "pwsh" => {
            ("powershell", arborium::lang_powershell::HIGHLIGHTS_QUERY)
        }
        "elixir" | "ex" | "exs" => ("elixir", arborium::lang_elixir::HIGHLIGHTS_QUERY),
        "sql" => ("sql", arborium::lang_sql::HIGHLIGHTS_QUERY),
        "starlark" | "bzl" | "bazel" => ("starlark", arborium::lang_starlark::HIGHLIGHTS_QUERY),
        "objectivec" | "objc" | "objective-c" => ("objc", &arborium::lang_objc::HIGHLIGHTS_QUERY),
        "vue" => ("vue", &arborium::lang_vue::HIGHLIGHTS_QUERY),
        "dockerfile" | "docker" => ("dockerfile", arborium::lang_dockerfile::HIGHLIGHTS_QUERY),
        "markdown" | "md" => ("markdown", arborium::lang_markdown::HIGHLIGHTS_QUERY),
        _ => return None,
    })
}

fn grammar(language: &str) -> Option<Arc<Grammar>> {
    let (name, query) = grammar_source(language)?;
    let mut grammars = GRAMMARS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    grammars
        .entry(name)
        .or_insert_with(|| {
            let language = arborium::get_language(name)?;
            let query = Query::new(&language, query).ok()?;
            let classes = query
                .capture_names()
                .iter()
                .map(|name| class(name))
                .collect();
            Some(Arc::new(Grammar {
                language,
                query,
                classes,
            }))
        })
        .clone()
}

fn class(capture: &str) -> Option<Highlight> {
    match capture {
        "text.title" => return Some(Highlight::Keyword),
        "text.literal" => return Some(Highlight::String),
        "text.uri" => return Some(Highlight::Function),
        "text.reference" => return Some(Highlight::Property),
        _ => {}
    }
    match capture.split('.').next() {
        Some("keyword") => Some(Highlight::Keyword),
        Some("function") => Some(Highlight::Function),
        Some("string") => Some(Highlight::String),
        Some("type") => Some(Highlight::Type),
        Some("number") => Some(Highlight::Number),
        Some("comment") | Some("punctuation") => Some(Highlight::Comment),
        Some("property") => Some(Highlight::Property),
        Some("tag") => Some(Highlight::Tag),
        _ => None,
    }
}

/// The code split into lines of spans, highlighted when the language is known.
pub fn highlight(code: &str, language: Option<&str>) -> Vec<Vec<CodeSpan>> {
    let classes = language
        .filter(|_| code.len() <= MAX_HIGHLIGHT_BYTES)
        .and_then(grammar)
        .and_then(|grammar| classify(code, &grammar));
    let mut lines = Vec::new();
    let mut offset = 0;
    for line in code.split('\n') {
        let mut spans: Vec<CodeSpan> = Vec::new();
        let mut start = 0;
        let class_at = |index: usize| classes.as_ref().and_then(|classes| classes[offset + index]);
        let bytes: Vec<(usize, char)> = line.char_indices().collect();
        for (position, (index, _)) in bytes.iter().enumerate() {
            let next = bytes
                .get(position + 1)
                .map_or(line.len(), |(next, _)| *next);
            let current = class_at(*index);
            let following = (next < line.len()).then(|| class_at(next));
            if following != Some(current) {
                spans.push(CodeSpan {
                    text: line[start..next].to_owned(),
                    highlight: current,
                });
                start = next;
            }
        }
        lines.push(spans);
        offset += line.len() + 1;
    }
    // A fence's code ends with a newline; drop the empty line after it.
    if lines.len() > 1 && lines.last().is_some_and(Vec::is_empty) {
        lines.pop();
    }
    lines
}

/// The class of every byte; later captures win, as in Warp's editor.
fn classify(code: &str, grammar: &Grammar) -> Option<Vec<Option<Highlight>>> {
    let mut parser = Parser::new();
    parser.set_language(&grammar.language).ok()?;
    let tree = parser.parse(code, None)?;
    let mut classes = vec![None; code.len()];
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(&grammar.query, tree.root_node(), code.as_bytes());
    while let Some((found, index)) = captures.next() {
        let capture = found.captures[*index];
        if let Some(class) = grammar
            .classes
            .get(capture.index as usize)
            .copied()
            .flatten()
        {
            let range = capture.node.byte_range();
            for slot in &mut classes[range.start.min(code.len())..range.end.min(code.len())] {
                *slot = Some(class);
            }
        }
    }
    Some(classes)
}
