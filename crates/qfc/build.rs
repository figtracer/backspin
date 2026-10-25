//! Builds FreeType 2.10.4 (third_party/freetype at VER-2-10-4, the version FL ships in
//! libfreetype_x64.dylib) as a static library with FreeType's default options and modules,
//! except zlib: `ftconfig/freetype/config/ftoption.h` shadows the stock header and leaves
//! `FT_CONFIG_OPTION_USE_ZLIB` undefined (the bundled zlib does not build with current SDKs, and
//! TrueType/OpenType fonts do not need it).

use std::path::PathBuf;

const SOURCES: &[&str] = &[
    "src/autofit/autofit.c",
    "src/base/ftbase.c",
    "src/base/ftbbox.c",
    "src/base/ftbdf.c",
    "src/base/ftbitmap.c",
    "src/base/ftcid.c",
    "src/base/ftfstype.c",
    "src/base/ftgasp.c",
    "src/base/ftglyph.c",
    "src/base/ftgxval.c",
    "src/base/ftinit.c",
    "src/base/ftmm.c",
    "src/base/ftotval.c",
    "src/base/ftpatent.c",
    "src/base/ftpfr.c",
    "src/base/ftstroke.c",
    "src/base/ftsynth.c",
    "src/base/ftsystem.c",
    "src/base/fttype1.c",
    "src/base/ftwinfnt.c",
    "src/base/ftdebug.c",
    "src/bdf/bdf.c",
    "src/bzip2/ftbzip2.c",
    "src/cache/ftcache.c",
    "src/cff/cff.c",
    "src/cid/type1cid.c",
    "src/gzip/ftgzip.c",
    "src/lzw/ftlzw.c",
    "src/pcf/pcf.c",
    "src/pfr/pfr.c",
    "src/psaux/psaux.c",
    "src/pshinter/pshinter.c",
    "src/psnames/psnames.c",
    "src/raster/raster.c",
    "src/sfnt/sfnt.c",
    "src/smooth/smooth.c",
    "src/truetype/truetype.c",
    "src/type1/type1.c",
    "src/type42/type42.c",
    "src/winfonts/winfnt.c",
];

fn main() {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ft = here.join("../../third_party/freetype");
    println!("cargo:rerun-if-changed=ftconfig/freetype/config/ftoption.h");
    let mut b = cc::Build::new();
    b.include(here.join("ftconfig"))
        .include(ft.join("include"))
        .define("FT2_BUILD_LIBRARY", None)
        .flag("-ffp-contract=off")
        .warnings(false);
    for s in SOURCES {
        b.file(ft.join(s));
    }
    b.compile("freetype");
}
