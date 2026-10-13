fn main() {
    println!("cargo:rustc-env=HARNESS_BUILD_OPT_LEVEL={}", std::env::var("OPT_LEVEL").unwrap_or_else(|_| "unknown".into()));
    println!("cargo:rustc-env=HARNESS_BUILD_PROFILE={}", std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into()));
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent-dark".into()),
    )
    .expect("No se pudo compilar la interfaz Slint");
}
