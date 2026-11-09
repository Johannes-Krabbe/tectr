//! Syntax highlighting shared by the apps. Code goes in and spans labelled by role come
//! out, so every app gets the same result and picks its own colors for each role.
//!
//! This can take a while on a long block, so call it off the UI thread.

use std::sync::OnceLock;

use serde::Serialize;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// What a piece of code is. Each role has a `color.syntax` or `color.diff` token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Plain,
    Keyword,
    String,
    Constant,
    Function,
    Type,
    Comment,
    Punctuation,
    Added,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Span {
    pub role: Role,
    pub text: String,
}

/// Grammar scopes and their roles. The first prefix that matches wins, so narrower
/// scopes come before the wider ones that contain them.
const ROLES: &[(&str, Role)] = &[
    // A quote belongs to its string, and a `//` to its comment.
    ("punctuation.definition.string", Role::String),
    ("punctuation.definition.comment", Role::Comment),
    ("punctuation.definition.inserted", Role::Added),
    ("punctuation.definition.deleted", Role::Deleted),
    ("punctuation", Role::Punctuation),
    ("comment", Role::Comment),
    ("string", Role::String),
    ("constant", Role::Constant),
    ("support.constant", Role::Constant),
    ("entity.name.function", Role::Function),
    ("support.function", Role::Function),
    ("variable.function", Role::Function),
    // `and`, `new`, and `typeof` read as keywords, while `+` and `=` are just punctuation.
    ("keyword.operator.word", Role::Keyword),
    ("keyword.operator.new", Role::Keyword),
    ("keyword.operator.expression", Role::Keyword),
    ("keyword.operator", Role::Punctuation),
    ("keyword", Role::Keyword),
    ("storage", Role::Keyword),
    ("variable.language", Role::Keyword),
    ("entity.name.tag", Role::Keyword),
    ("entity.name.type", Role::Type),
    ("entity.name.class", Role::Type),
    ("entity.name.struct", Role::Type),
    ("entity.name.enum", Role::Type),
    ("entity.name.trait", Role::Type),
    ("entity.name.interface", Role::Type),
    ("entity.other.inherited-class", Role::Type),
    ("entity.other.attribute-name", Role::Type),
    ("support.type", Role::Type),
    ("support.class", Role::Type),
    ("markup.inserted", Role::Added),
    ("markup.deleted", Role::Deleted),
];

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

fn role_scopes() -> &'static [(Scope, Role)] {
    static SCOPES: OnceLock<Vec<(Scope, Role)>> = OnceLock::new();
    SCOPES.get_or_init(|| {
        ROLES
            .iter()
            .map(|&(scope, role)| (Scope::new(scope).expect("scopes above are valid"), role))
            .collect()
    })
}

/// The innermost scope that has a role decides.
fn role_of(stack: &ScopeStack) -> Role {
    stack
        .as_slice()
        .iter()
        .rev()
        .find_map(|scope| {
            role_scopes()
                .iter()
                .find(|(prefix, _)| prefix.is_prefix_of(*scope))
                .map(|&(_, role)| role)
        })
        .unwrap_or(Role::Plain)
}

fn push(spans: &mut Vec<Span>, role: Role, text: &str) {
    if text.is_empty() {
        return;
    }
    match spans.last_mut() {
        Some(last) if last.role == role => last.text.push_str(text),
        _ => spans.push(Span {
            role,
            text: text.to_owned(),
        }),
    }
}

/// Splits a line at each change of scope, moving `stack` along. Writes nothing on failure.
fn highlight_line(
    line: &str,
    state: &mut ParseState,
    stack: &mut ScopeStack,
    spans: &mut Vec<Span>,
) -> Option<()> {
    let ops = state.parse_line(line, syntaxes()).ok()?;
    let mut line_spans = Vec::new();
    let mut start = 0;
    for (end, op) in &ops {
        push(&mut line_spans, role_of(stack), line.get(start..*end)?);
        start = *end;
        stack.apply(op).ok()?;
    }
    push(&mut line_spans, role_of(stack), line.get(start..)?);
    for span in line_spans {
        push(spans, span.role, &span.text);
    }
    Some(())
}

/// Finds a grammar by what follows a code fence. Grammars answer to their name and file
/// extensions, which misses a few names people write.
fn find_syntax(language: &str) -> Option<&'static SyntaxReference> {
    let token = language.split(',').next().unwrap_or_default().trim();
    let token = match token.to_ascii_lowercase().as_str() {
        "jsx" => "tsx",
        "mjs" => "js",
        "jsonc" => "json",
        "golang" => "go",
        "csharp" => "cs",
        "shell" | "console" => "bash",
        "docker" => "dockerfile",
        _ => token,
    };
    syntaxes().find_syntax_by_token(token)
}

/// Splits `code` by role. `language` is what follows a code fence, such as `rust`, `ts`,
/// or `rust,ignore`. Joined back together, the spans are always exactly `code`: a language
/// nobody knows is one plain span, and a grammar that fails leaves the rest plain.
pub fn highlight(code: &str, language: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let Some(syntax) = find_syntax(language) else {
        push(&mut spans, Role::Plain, code);
        return spans;
    };

    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut done = 0;
    for line in LinesWithEndings::from(code) {
        if highlight_line(line, &mut state, &mut stack, &mut spans).is_none() {
            break;
        }
        done += line.len();
    }
    push(&mut spans, Role::Plain, &code[done..]);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The role of the first span holding `text`, and that the spans still add up to `code`.
    fn role(code: &str, language: &str, text: &str) -> Role {
        let spans = highlight(code, language);
        let joined: String = spans.iter().map(|span| span.text.as_str()).collect();
        assert_eq!(joined, code);
        spans
            .iter()
            .find(|span| span.text.contains(text))
            .unwrap_or_else(|| panic!("no span holds {text:?} in {spans:?}"))
            .role
    }

    #[test]
    fn labels_rust_by_role() {
        let code = "// note\nfn add(a: u8) -> Vec<u8> {\n    let s = \"hi\";\n    s.len() + 1\n}\n";
        assert_eq!(role(code, "rust", "// note"), Role::Comment);
        assert_eq!(role(code, "rust", "fn"), Role::Keyword);
        assert_eq!(role(code, "rust", "add"), Role::Function);
        assert_eq!(role(code, "rust", "Vec"), Role::Type);
        assert_eq!(role(code, "rust", "let"), Role::Keyword);
        assert_eq!(role(code, "rust", "1"), Role::Constant);
    }

    #[test]
    fn a_quote_belongs_to_its_string() {
        assert_eq!(role("let s = \"hi\";\n", "rust", "\"hi\""), Role::String);
    }

    #[test]
    fn finds_languages_by_their_short_names() {
        for language in ["ts", "tsx", "typescript", "js", "jsx"] {
            assert_eq!(
                role("const a = 1;\n", language, "const"),
                Role::Keyword,
                "{language}"
            );
        }
        assert_eq!(role("def f():\n    pass\n", "py", "def"), Role::Keyword);
        assert_eq!(
            role("if true; then\n  echo hi\nfi\n", "sh", "if"),
            Role::Keyword
        );
        assert_eq!(
            role("if true; then\n  echo hi\nfi\n", "bash", "then"),
            Role::Keyword
        );
        assert_eq!(role("a = \"b\"\n", "toml", "\"b\""), Role::String);
        assert_eq!(role("{\"a\": 1}\n", "json", "1"), Role::Constant);
        assert_eq!(role("a: 1\n", "yaml", "1"), Role::Constant);
        assert_eq!(role("fn a() {}\n", "Rust", "fn"), Role::Keyword);
    }

    #[test]
    fn knows_the_names_agents_write() {
        let missing: Vec<_> = [
            "rust",
            "rs",
            "ts",
            "typescript",
            "tsx",
            "js",
            "javascript",
            "jsx",
            "mjs",
            "py",
            "python",
            "go",
            "golang",
            "java",
            "kotlin",
            "kt",
            "swift",
            "c",
            "cpp",
            "c++",
            "cs",
            "csharp",
            "rb",
            "ruby",
            "php",
            "sh",
            "bash",
            "zsh",
            "shell",
            "console",
            "fish",
            "sql",
            "html",
            "xml",
            "css",
            "scss",
            "json",
            "jsonc",
            "yaml",
            "yml",
            "toml",
            "ini",
            "md",
            "markdown",
            "dockerfile",
            "docker",
            "makefile",
            "make",
            "diff",
            "patch",
            "lua",
            "elixir",
            "hs",
            "haskell",
            "zig",
            "nix",
            "graphql",
            "proto",
            "hcl",
            "vue",
            "svelte",
        ]
        .into_iter()
        .filter(|name| find_syntax(name).is_none())
        .collect();
        assert_eq!(missing, Vec::<&str>::new());
    }

    #[test]
    fn new_reads_as_a_keyword() {
        assert_eq!(role("x = new Foo();\n", "ts", "new"), Role::Keyword);
    }

    #[test]
    fn ignores_what_follows_the_language() {
        assert_eq!(role("fn a() {}\n", "rust,ignore", "fn"), Role::Keyword);
    }

    #[test]
    fn labels_diffs() {
        let code = "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n";
        assert_eq!(role(code, "diff", "old"), Role::Deleted);
        assert_eq!(role(code, "diff", "new"), Role::Added);
    }

    #[test]
    fn an_unknown_language_is_plain() {
        assert_eq!(
            highlight("anything", "not-a-language"),
            [Span {
                role: Role::Plain,
                text: "anything".to_owned(),
            }],
        );
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert_eq!(highlight("", "rust"), []);
        assert_eq!(highlight("", "not-a-language"), []);
    }

    #[test]
    fn handles_code_cut_off_mid_stream() {
        for code in ["fn", "fn main(", "let s = \"unfinished", "/* open", "é \"ü"] {
            let joined: String = highlight(code, "rust")
                .iter()
                .map(|s| s.text.as_str())
                .collect();
            assert_eq!(joined, code);
        }
    }

    #[test]
    fn every_role_scope_is_valid() {
        assert_eq!(role_scopes().len(), ROLES.len());
    }
}
