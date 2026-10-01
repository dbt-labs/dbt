#![cfg(feature = "unstable_machinery")]
use minijinja::{
    machinery::{tokenize, Span, Token, WhitespaceConfig},
    syntax::SyntaxConfig,
};

use std::fmt::Write;

use serde::Deserialize;

#[derive(Deserialize, Default)]
#[serde(default)]
struct TestSettings {
    keep_trailing_newline: bool,
    lstrip_blocks: bool,
    trim_blocks: bool,
    markers: Option<[String; 6]>,
    line_statement_prefix: Option<String>,
    line_comment_prefix: Option<String>,
}

impl TestSettings {
    pub fn into_configs(self) -> (SyntaxConfig, WhitespaceConfig) {
        use minijinja::syntax::SyntaxConfig;
        let mut builder = SyntaxConfig::builder();
        if let Some(ref markers) = self.markers {
            builder
                .block_delimiters(markers[0].to_string(), markers[1].to_string())
                .variable_delimiters(markers[2].to_string(), markers[3].to_string())
                .comment_delimiters(markers[4].to_string(), markers[5].to_string());
        }
        if let Some(prefix) = self.line_statement_prefix {
            builder.line_statement_prefix(prefix);
        }
        if let Some(prefix) = self.line_comment_prefix {
            builder.line_comment_prefix(prefix);
        }
        (
            builder.build().unwrap(),
            WhitespaceConfig {
                keep_trailing_newline: self.keep_trailing_newline,
                lstrip_blocks: self.lstrip_blocks,
                trim_blocks: self.trim_blocks,
            },
        )
    }
}

fn stringify_tokens(tokens: Vec<(Token<'_>, Span)>, contents: &str) -> String {
    let mut stringified = String::new();
    for (token, span) in tokens {
        let token_source = contents
            .get(span.start_offset as usize..span.end_offset as usize)
            .unwrap();
        writeln!(stringified, "{token:?}").unwrap();
        writeln!(stringified, "  {token_source:?}").unwrap();
    }
    stringified
}

#[test]
fn test_lexer() {
    insta::glob!("lexer-inputs/*.txt", |path| {
        let contents = std::fs::read_to_string(path).unwrap();
        let mut iter = contents.splitn(2, "\n---\n");
        let settings: TestSettings = serde_json::from_str(iter.next().unwrap()).unwrap();
        let (syntax_config, whitespace_config) = settings.into_configs();
        let contents = iter.next().unwrap();
        let tokens: Result<Vec<_>, _> =
            tokenize(contents, false, syntax_config, whitespace_config).collect();
        insta::with_settings!({
            description => contents.trim_end(),
            omit_expression => true
        }, {
            let stringified = stringify_tokens(tokens.unwrap(), contents);
            insta::assert_snapshot!(&stringified);
        });
    });
}

#[test]
fn test_trim_blocks() {
    let input = "{% block foo %}\nbar{% endblock %}";
    let tokens: Result<Vec<_>, _> = tokenize(
        input,
        false,
        Default::default(),
        WhitespaceConfig {
            trim_blocks: true,
            ..Default::default()
        },
    )
    .collect();
    let stringified = stringify_tokens(tokens.unwrap(), input);
    insta::assert_snapshot!(&stringified, @r###"
    BlockStart
      "{%"
    Ident("block")
      "block"
    Ident("foo")
      "foo"
    BlockEnd
      "%}"
    TemplateData("bar")
      "bar"
    BlockStart
      "{%"
    Ident("endblock")
      "endblock"
    BlockEnd
      "%}"
    "###);
}
