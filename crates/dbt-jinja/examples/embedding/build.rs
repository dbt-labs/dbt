fn main() {
    // we only need to bundle the templates with the
    // feature is enabled.
    #[cfg(feature = "bundled")]
    {
        minijinja_embed::embed_templates!("src/templates");
    }
}
