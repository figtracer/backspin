//! The FreeType 2.10.4 entry points and records QuickFontCache uses. The records are the public
//! prefixes of the FreeType structs (`FT_Long` is a C `long`: 64-bit on macOS, 32-bit on
//! Windows).

use std::os::raw::{c_char, c_int, c_long, c_short, c_uchar, c_uint, c_ulong, c_ushort, c_void};

pub type FtLibrary = *mut c_void;
pub type FtFace = *mut FaceRec;
pub type FtError = c_int;

/// `FT_KERNING_DEFAULT`.
pub const KERNING_DEFAULT: c_uint = 0;
/// `FT_ENCODING_UNICODE` ('unic').
pub const ENCODING_UNICODE: c_int = 0x756e6963;
/// `FT_ENCODING_MS_SYMBOL` ('symb').
pub const ENCODING_MS_SYMBOL: c_int = 0x73796d62;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Vector {
    pub x: c_long,
    pub y: c_long,
}

#[repr(C)]
pub struct Generic {
    pub data: *mut c_void,
    pub finalizer: *mut c_void,
}

#[repr(C)]
pub struct BBox {
    pub x_min: c_long,
    pub y_min: c_long,
    pub x_max: c_long,
    pub y_max: c_long,
}

#[repr(C)]
pub struct GlyphMetrics {
    pub width: c_long,
    pub height: c_long,
    pub hori_bearing_x: c_long,
    pub hori_bearing_y: c_long,
    pub hori_advance: c_long,
    pub vert_bearing_x: c_long,
    pub vert_bearing_y: c_long,
    pub vert_advance: c_long,
}

#[repr(C)]
pub struct Bitmap {
    pub rows: c_uint,
    pub width: c_uint,
    pub pitch: c_int,
    pub buffer: *mut c_uchar,
    pub num_grays: c_ushort,
    pub pixel_mode: c_uchar,
    pub palette_mode: c_uchar,
    pub palette: *mut c_void,
}

#[repr(C)]
pub struct Outline {
    pub n_contours: c_short,
    pub n_points: c_short,
    pub points: *mut Vector,
    pub tags: *mut c_char,
    pub contours: *mut c_short,
    pub flags: c_int,
}

#[repr(C)]
pub struct GlyphSlotRec {
    pub library: FtLibrary,
    pub face: FtFace,
    pub next: *mut GlyphSlotRec,
    pub glyph_index: c_uint,
    pub generic: Generic,
    pub metrics: GlyphMetrics,
    pub linear_hori_advance: c_long,
    pub linear_vert_advance: c_long,
    pub advance: Vector,
    pub format: c_int,
    pub bitmap: Bitmap,
    pub bitmap_left: c_int,
    pub bitmap_top: c_int,
    pub outline: Outline,
}

#[repr(C)]
pub struct SizeMetrics {
    pub x_ppem: c_ushort,
    pub y_ppem: c_ushort,
    pub x_scale: c_long,
    pub y_scale: c_long,
    pub ascender: c_long,
    pub descender: c_long,
    pub height: c_long,
    pub max_advance: c_long,
}

#[repr(C)]
pub struct SizeRec {
    pub face: FtFace,
    pub generic: Generic,
    pub metrics: SizeMetrics,
}

#[repr(C)]
pub struct CharMapRec {
    pub face: FtFace,
    pub encoding: c_int,
    pub platform_id: c_ushort,
    pub encoding_id: c_ushort,
}

#[repr(C)]
pub struct FaceRec {
    pub num_faces: c_long,
    pub face_index: c_long,
    pub face_flags: c_long,
    pub style_flags: c_long,
    pub num_glyphs: c_long,
    pub family_name: *mut c_char,
    pub style_name: *mut c_char,
    pub num_fixed_sizes: c_int,
    pub available_sizes: *mut c_void,
    pub num_charmaps: c_int,
    pub charmaps: *mut *mut CharMapRec,
    pub generic: Generic,
    pub bbox: BBox,
    pub units_per_em: c_ushort,
    pub ascender: c_short,
    pub descender: c_short,
    pub height: c_short,
    pub max_advance_width: c_short,
    pub max_advance_height: c_short,
    pub underline_position: c_short,
    pub underline_thickness: c_short,
    pub glyph: *mut GlyphSlotRec,
    pub size: *mut SizeRec,
    pub charmap: *mut CharMapRec,
}

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
    pub fn FT_Select_Charmap(face: FtFace, encoding: c_int) -> FtError;
    pub fn FT_Set_Pixel_Sizes(face: FtFace, w: c_uint, h: c_uint) -> FtError;
    pub fn FT_Get_Char_Index(face: FtFace, code: c_ulong) -> c_uint;
    pub fn FT_Load_Glyph(face: FtFace, index: c_uint, flags: i32) -> FtError;
    pub fn FT_Get_Kerning(
        face: FtFace,
        left: c_uint,
        right: c_uint,
        mode: c_uint,
        out: *mut Vector,
    ) -> FtError;
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
