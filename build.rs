fn main() {
    // Only the desktop app has a UI to compile; the library and the web server do not need Slint.
    #[cfg(feature = "desktop")]
    {
        let config = slint_build::CompilerConfiguration::new().embed_resources(slint_build::EmbedResourcesKind::EmbedFiles).with_style("fluent".into());
        slint_build::compile_with_config("slint/app.slint", config).expect("the Slint UI did not compile");
    }
}
