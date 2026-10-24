fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("cupertino".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");

    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=app.manifest");
        println!("cargo:rerun-if-changed=../../assets/app.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/app.ico")
            .set_manifest_file("app.manifest")
            .set("FileDescription", "Keeplight")
            .set("ProductName", "Keeplight")
            .set("LegalCopyright", "GPL-3.0-only");
        res.compile().expect("failed to embed Windows resources");
    }
}
