#[cfg(feature = "vendored")]
fn build_lib() {
    use std::path::PathBuf;

    let vendor = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor");
    let units_dir = vendor.join("units");

    println!("cargo:rerun-if-changed={}", units_dir.display());

    let mut build = cc::Build::new();
    build
        .include(&units_dir)
        .file(units_dir.join("units.c"))
        .file(units_dir.join("parse.tab.c"))
        .file(units_dir.join("getopt.c"))
        .file(units_dir.join("getopt1.c"))
        .file(units_dir.join("strfunc.c"))
        .flag("-Wno-error=int-conversion")
        .define("main", "static gnu_units_main")
        .warnings(false)
        .compile("gnu_units");
}

#[cfg(feature = "bindgen")]
fn generate_bindings() {
    use std::path::PathBuf;

    println!("cargo:rerun-if-changed=wrapper.h");

    let builder = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg("-Ivendor/units");

    let bindings = builder
        .allowlist_function("newunit")
        .allowlist_function("newprefix")
        .allowlist_function("newtable")
        .allowlist_function("newfunction")
        .allowlist_function("newalias")
        .allowlist_function("initializeunit")
        .allowlist_function("freeunit")
        .allowlist_function("unitcopy")
        .allowlist_function("parseunit")
        .allowlist_function("unit2num")
        .allowlist_function("evalfunc")
        .allowlist_function("fnlookup")
        .allowlist_function("completereduce")
        .allowlist_function("addunit")
        .allowlist_function("multunit")
        .allowlist_function("divunit")
        .allowlist_function("invertunit")
        .allowlist_function("expunit")
        .allowlist_function("rootunit")
        .allowlist_var("mylocale")
        .allowlist_var("progname")
        .allowlist_var("utf8mode")
        .allowlist_var("E_.*")
        .allowlist_var("COMP_.*")
        .allowlist_var("MAXSUBUNITS")
        .allowlist_var("MAX_FUNC_PARAMS")
        .allowlist_var("NULLUNIT")
        .allowlist_var("lastunit")
        .allowlist_var("lastunitset")
        .allowlist_type("unittype")
        .allowlist_type("interval")
        .allowlist_type("functype")
        .allowlist_type("pair")
        .allowlist_type("func")
        .allowlist_type("parseflag")
        .generate()
        .unwrap_or_else(|err| {
            eprintln!("Failed to generate bindings to GNU units: {err}");
            std::process::exit(1);
        });

    let out_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("gen/bindings.rs");

    bindings.write_to_file(&out_path).unwrap_or_else(|err| {
        eprintln!(
            "Failed to write GNU units bindings to {:?}: {}",
            out_path.display(),
            err,
        );
        std::process::exit(1);
    });
}

fn main() {
    build_lib();

    #[cfg(feature = "bindgen")]
    generate_bindings();

    if cfg!(target_family = "unix") {
        println!("cargo:rustc-link-lib=m");
    }
}
