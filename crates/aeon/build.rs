fn main() {
    println!("cargo:rerun-if-changed=aeon.rc");
    println!("cargo:rerun-if-changed=../../packaging/windows/aeon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("aeon.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("compiling aeon.rc (needs the Windows SDK resource compiler)");
    }
}
