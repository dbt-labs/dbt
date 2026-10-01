#![cfg(feature = "unstable_machinery")]
use minijinja::machinery::parse;

#[test]
fn test_parser() {
    insta::glob!("parser-inputs/*.txt", |path| {
        let contents = std::fs::read_to_string(path).unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        let ast = parse(&contents, filename, Default::default(), Default::default());
        insta::with_settings!({
            description => contents.trim_end(),
            omit_expression => true,
        }, {
            insta::assert_debug_snapshot!(&ast);
        });
    });
}
