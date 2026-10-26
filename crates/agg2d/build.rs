//! Builds Blend2D 0.11.1 (third_party/blend2d at a7f9476, with AsmJit 1.13 at d6c5be2 for the JIT
//! pipelines, as FL's libblend2d.dylib, without FP contraction) as a static library, then the Agg2D wrapper on top of it.

use std::path::PathBuf;

fn main() {
    let msvc = std::env::var("TARGET").unwrap().contains("msvc");
    // No FP contraction: GCC/Clang contract by default; MSVC does not under /fp:precise.
    let no_contract = if msvc {
        "/fp:precise"
    } else {
        "-ffp-contract=off"
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let blend2d = root.join("third_party/blend2d");
    let asmjit = root.join("third_party/asmjit");
    println!("cargo:rerun-if-changed=cpp/agg2d.cpp");
    println!("cargo:rerun-if-changed=cpp/agg2d.h");

    let dst = cmake::Config::new(&blend2d)
        .define("BLEND2D_STATIC", "ON")
        .define("BLEND2D_TEST", "OFF")
        .define("BLEND2D_DEMOS", "OFF")
        .define("BLEND2D_NO_INSTALL", "OFF")
        .define("ASMJIT_DIR", &asmjit)
        .profile("Release")
        // FL's build does not contract floating point (vertices of arcs match only this way).
        .cxxflag(no_contract)
        .build();
    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=blend2d");

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("cpp/agg2d.cpp")
        .include(dst.join("include"))
        .define("BL_STATIC", None)
        .flag(no_contract)
        .flag_if_supported("-fno-exceptions")
        .flag_if_supported("-fno-rtti")
        .compile("agg2d");
    let target = std::env::var("TARGET").unwrap();
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=c++");
    } else if !msvc {
        println!("cargo:rustc-link-lib=stdc++");
    }
}
