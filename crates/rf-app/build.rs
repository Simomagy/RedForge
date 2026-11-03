//! The Windows exe's icon, the C# RedForge's.

fn main() {
    println!("cargo:rerun-if-changed=assets/redforge.rc");
    println!("cargo:rerun-if-changed=assets/redforge.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/redforge.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("the icon resource");
    }
}
