//! The FreeType 2.10.4 entry points QuickFontCache uses.

use std::os::raw::{c_int, c_long, c_void};

pub type FtLibrary = *mut c_void;
pub type FtFace = *mut c_void;
pub type FtError = c_int;

unsafe extern "C" {
    pub fn FT_Init_FreeType(lib: *mut FtLibrary) -> FtError;
    pub fn FT_Done_FreeType(lib: FtLibrary) -> FtError;
    pub fn FT_Library_Version(
        lib: FtLibrary,
        major: *mut c_int,
        minor: *mut c_int,
        patch: *mut c_int,
    );
    pub fn FT_New_Memory_Face(
        lib: FtLibrary,
        base: *const u8,
        size: c_long,
        index: c_long,
        face: *mut FtFace,
    ) -> FtError;
    pub fn FT_Done_Face(face: FtFace) -> FtError;
}

/// The version of the linked FreeType.
pub fn version() -> (i32, i32, i32) {
    let mut lib = std::ptr::null_mut();
    let (mut a, mut b, mut c) = (0, 0, 0);
    unsafe {
        FT_Init_FreeType(&mut lib);
        FT_Library_Version(lib, &mut a, &mut b, &mut c);
        FT_Done_FreeType(lib);
    }
    (a, b, c)
}
