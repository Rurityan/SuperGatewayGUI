fn main() {
    embed_resource::compile_for(
        "assets/app.rc",
        ["SuperGatewayGUI"],
        embed_resource::ParamsIncludeDirs(["assets"]),
    )
    .manifest_required()
    .expect("Compile embedded Windows application icon");
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("app.manifest");
    println!("cargo:rustc-link-arg-bin=SuperGatewayGUI=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=SuperGatewayGUI=/MANIFESTINPUT:{}",
        manifest.display()
    );
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
}
