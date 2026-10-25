// Image-Line's Agg2D wrapper over Blend2D, ported from FL/Shared/libblend2d.dylib (arm64,
// `(anonymous namespace)::Agg2D`, addresses bl:0x1ba0..0x4200, see .amp/in/gfx/notes/gfx.md section 4).
// Field names follow the original layout; offsets are given where the decompile uses them.
// Only the C API of Blend2D is used so that every call matches the original one for one.

#include <blend2d.h>
#include <stdint.h>
#include <string.h>

namespace {

// CustomStrokeContext (Agg2D + 0x70): two stroke option sets and four scratch paths.
struct CustomStrokeContext {
  const BLPathCore* input;      // +0x00
  BLStrokeOptionsCore outer;    // +0x08, width 2 * (offset - w)
  BLStrokeOptionsCore inner;    // +0x38, width 2 * w
  BLPathCore p68, p78, p88, p98;

  static BLResult appendOuterStrokeSink(BLPathCore*, BLPathCore*, BLPathCore*, size_t, size_t, void*) noexcept {
    return BL_SUCCESS;
  }

  static BLResult appendCustomStrokeSink2(BLPathCore* a, BLPathCore* b, BLPathCore* c, size_t s, size_t e,
                                          void* closure) noexcept {
    (void)a; (void)c; (void)s; (void)e;
    return blPathAddReversedPath(static_cast<BLPathCore*>(closure), b, nullptr, BL_PATH_REVERSE_MODE_COMPLETE);
  }

  static BLResult appendCustomStrokeSink(BLPathCore* a, BLPathCore* b, BLPathCore* c, size_t inputStart,
                                         size_t inputEnd, void* closure) noexcept {
    (void)b; (void)c;
    CustomStrokeContext* self = static_cast<CustomStrokeContext*>(closure);
    blPathClear(&self->p98);
    BLRange range = {inputStart, inputEnd};
    return blPathStrokeToSink(self->input, &range, &self->inner, &blDefaultApproximationOptions, &self->p98,
                              &self->p68, &self->p88, appendCustomStrokeSink2, a);
  }

  // bl:0x3d24 CustomStrokeContext::stroke(out, in, w, d, join).
  void stroke(BLPathCore* out, const BLPathCore* in, double w, double d, uint8_t join) {
    outer.join = join;
    inner.join = join;
    outer.width = d + d;
    inner.width = w + w;
    if (d == -1.0) {
      blPathClear(&p68);
      blPathAddReversedPath(&p68, in, nullptr, BL_PATH_REVERSE_MODE_COMPLETE);
      blPathStrokeToSink(&p68, nullptr, &inner, &blDefaultApproximationOptions, out, &p78, &p88,
                         appendOuterStrokeSink, this);
    } else {
      input = in;
      blPathStrokeToSink(in, nullptr, &outer, &blDefaultApproximationOptions, out, &p68, &p78,
                         appendCustomStrokeSink, this);
      input = nullptr;
    }
  }
};

enum : uint8_t { STYLE_SOLID = 0, STYLE_NONE = 1, STYLE_GRADIENT = 2 };

struct Agg2D {
  BLImageCore image;          // +0x08
  BLContextCore ctx;          // +0x18
  uint8_t transformDirty;     // +0x58
  uint8_t customStroke;       // +0x59
  uint8_t gradientDirty[2];   // +0x5a fill, +0x5b stroke
  uint8_t style[2];           // +0x5c fill, +0x5d stroke
  double csWidth;             // +0x60
  double csOffset;            // +0x68
  CustomStrokeContext cs;     // +0x70
  uint32_t color[2];          // +0x118 fill, +0x11c stroke (BLRgba32 0xAARRGGBB)
  BLGradientCore gradient[2]; // +0x120 fill, +0x130 stroke
  BLPathCore path;            // +0x140 working path
  BLPathCore strokeOut;       // +0x150
  BLPathCore geometry;        // +0x160
  int16_t lut[2][256][4];     // +0x170 fill, +0x970 stroke: R, G, B, A in 1/128 units

  const BLContextState* state() const { return ctx._d.impl ? static_cast<const BLContextImpl*>(ctx._d.impl)->state : nullptr; }

  // bl:0x3e14 / 0x3b04 / 0x4058: the 256 gradient stops built from the colour buffer.
  void assignStops(int slot) {
    BLGradientStop stops[256];
    for (int i = 0; i < 256; i++) {
      const int16_t* c = lut[slot][i];
      uint64_t r = c[0] < 1 ? 0 : uint64_t(int64_t(c[0])) << 33;
      uint64_t g = c[1] < 1 ? 0 : (uint64_t(int64_t(c[1])) & 0x7fffffffu) << 17;
      uint64_t b = c[2] < 1 ? 0 : uint64_t(uint32_t(int32_t(c[2]) << 1));
      uint64_t a = c[3] < 1 ? 0 : uint64_t(uint16_t(c[3])) << 49;
      stops[i].offset = double(i) * 0.00392156862745098;
      stops[i].rgba.value = r | g | b | a;
    }
    blGradientAssignStops(&gradient[slot], stops, 256);
    gradientDirty[slot] = 0;
  }

  void fillPath(const BLPathCore* p, int slot) {
    if (style[slot] == STYLE_GRADIENT) {
      if (gradientDirty[slot])
        assignStops(slot);
      BLContextImpl* impl = static_cast<BLContextImpl*>(ctx._d.impl);
      impl->virt->setStyle(impl, BL_CONTEXT_STYLE_SLOT_FILL, reinterpret_cast<const BLObjectCore*>(&gradient[slot]),
                           BL_CONTEXT_STYLE_TRANSFORM_MODE_NONE);
      impl->virt->fillGeometry(impl, BL_GEOMETRY_TYPE_PATH, p);
    } else if (style[slot] == STYLE_SOLID) {
      BLContextImpl* impl = static_cast<BLContextImpl*>(ctx._d.impl);
      impl->virt->fillGeometryRgba32(impl, BL_GEOMETRY_TYPE_PATH, p, color[slot]);
    }
  }

  void strokePath(const BLPathCore* p, int slot) {
    if (!customStroke) {
      BLContextImpl* impl = static_cast<BLContextImpl*>(ctx._d.impl);
      if (style[slot] == STYLE_GRADIENT) {
        if (gradientDirty[slot])
          assignStops(slot);
        impl->virt->setStyle(impl, BL_CONTEXT_STYLE_SLOT_STROKE, reinterpret_cast<const BLObjectCore*>(&gradient[slot]),
                             BL_CONTEXT_STYLE_TRANSFORM_MODE_NONE);
        impl->virt->strokeGeometry(impl, BL_GEOMETRY_TYPE_PATH, p);
      } else if (style[slot] == STYLE_SOLID) {
        impl->virt->strokeGeometryRgba32(impl, BL_GEOMETRY_TYPE_PATH, p, color[slot]);
      }
    } else {
      blPathClear(&strokeOut);
      const BLContextState* s = state();
      cs.stroke(&strokeOut, p, csWidth, csOffset, s ? s->strokeOptions.join : 0);
      fillPath(&strokeOut, slot);
    }
  }
};

void lineColorSep(Agg2D* a, uint8_t r, uint8_t g, uint8_t b, uint8_t al) {
  a->style[1] = STYLE_SOLID;
  a->color[1] = uint32_t(r) << 16 | uint32_t(g) << 8 | b | uint32_t(al) << 24;
}

void fillColorSep(Agg2D* a, uint8_t r, uint8_t g, uint8_t b, uint8_t al) {
  a->style[0] = STYLE_SOLID;
  a->color[0] = uint32_t(r) << 16 | uint32_t(g) << 8 | b | uint32_t(al) << 24;
}

uint32_t swapRgba(int32_t c) {
  uint32_t u = uint32_t(c);
  return (u & 0xff000000u) | (u & 0xff00u) | (u & 0xffu) << 16 | (u >> 16 & 0xffu);
}

} // namespace

extern "C" {

void agg_line_width(Agg2D* a, double w);
void agg_line_cap(Agg2D* a, int cap);
void agg_line_join(Agg2D* a, int join);
void agg_fill_even_odd(Agg2D* a, int even_odd);
void agg_reset_transformations(Agg2D* a);

// bl:0x376c resetStuff (the constructor runs it before attach, so the context calls do nothing).
static void resetStuff(Agg2D* a) {
  agg_reset_transformations(a);
  agg_line_width(a, 1.0);
  lineColorSep(a, 0, 0, 0, 0xff);
  fillColorSep(a, 0xff, 0xff, 0xff, 0xff);
  agg_line_cap(a, 2);
  agg_line_join(a, 2);
  agg_fill_even_odd(a, 0);
  blPathClear(&a->path);
}

// bl:0x1ba0 createAgg2DWithBlend2D.
Agg2D* agg_new(void) {
  Agg2D* a = new Agg2D();
  blImageInit(&a->image);
  blContextInit(&a->ctx);
  a->transformDirty = 0;
  a->customStroke = 0;
  a->gradientDirty[0] = a->gradientDirty[1] = 0;
  a->style[0] = a->style[1] = 0;
  a->csWidth = 0;
  a->csOffset = 0;
  a->cs.input = nullptr;
  blStrokeOptionsInit(&a->cs.outer);
  blStrokeOptionsInit(&a->cs.inner);
  blPathInit(&a->cs.p68);
  blPathInit(&a->cs.p78);
  blPathInit(&a->cs.p88);
  blPathInit(&a->cs.p98);
  a->color[0] = a->color[1] = 0;
  blGradientInit(&a->gradient[0]);
  blGradientInit(&a->gradient[1]);
  blPathInit(&a->path);
  blPathInit(&a->strokeOut);
  blPathInit(&a->geometry);
  memset(a->lut, 0, sizeof(a->lut));
  resetStuff(a);
  return a;
}

// bl:0x1d48 destroyObject.
void agg_free(Agg2D* a) {
  if (!a)
    return;
  blPathDestroy(&a->geometry);
  blPathDestroy(&a->strokeOut);
  blPathDestroy(&a->path);
  blGradientDestroy(&a->gradient[1]);
  blGradientDestroy(&a->gradient[0]);
  blPathDestroy(&a->cs.p98);
  blPathDestroy(&a->cs.p88);
  blPathDestroy(&a->cs.p78);
  blPathDestroy(&a->cs.p68);
  blStrokeOptionsDestroy(&a->cs.inner);
  blStrokeOptionsDestroy(&a->cs.outer);
  blContextDestroy(&a->ctx);
  blImageDestroy(&a->image);
  delete a;
}

// bl:0x1df4 attach: PRGB32 image over the caller's pixels, even-odd fill, dithered gradients.
void agg_attach(Agg2D* a, uint8_t* bits, uint32_t w, uint32_t h, int32_t stride) {
  blImageCreateFromData(&a->image, int(w), int(h), BL_FORMAT_PRGB32, bits, intptr_t(stride),
                        BLDataAccessFlags(BL_DATA_ACCESS_RW), nullptr, nullptr);
  BLContextCreateInfo cci;
  memset(&cci, 0, sizeof(cci));
  blContextBegin(&a->ctx, &a->image, &cci);
  BLContextImpl* impl = static_cast<BLContextImpl*>(a->ctx._d.impl);
  impl->virt->setFillRule(impl, BL_FILL_RULE_EVEN_ODD);
  impl->virt->setHint(impl, BL_CONTEXT_HINT_GRADIENT_QUALITY, BL_GRADIENT_QUALITY_DITHER);
}

// Ends the context (the original keeps it until the next attach or destroy; the port detaches
// before the DIB memory goes away).
void agg_detach(Agg2D* a) {
  blContextEnd(&a->ctx);
  blImageReset(&a->image);
}

void agg_fill_color(Agg2D* a, int32_t c) {
  a->style[0] = STYLE_SOLID;
  a->color[0] = swapRgba(c);
}

void agg_fill_color_sep(Agg2D* a, uint8_t r, uint8_t g, uint8_t b, uint8_t al) { fillColorSep(a, r, g, b, al); }

void agg_no_fill(Agg2D* a) {
  a->style[0] = STYLE_NONE;
  a->color[0] = 0;
}

void agg_line_color(Agg2D* a, int32_t c) {
  a->style[1] = STYLE_SOLID;
  a->color[1] = swapRgba(c);
}

void agg_line_color_sep(Agg2D* a, uint8_t r, uint8_t g, uint8_t b, uint8_t al) { lineColorSep(a, r, g, b, al); }

void agg_no_line(Agg2D* a) {
  a->style[1] = STYLE_NONE;
  a->color[1] = 0;
}

// bl:0x2334.
void agg_line_width(Agg2D* a, double w) {
  blContextSetStrokeWidth(&a->ctx, w);
  a->customStroke = 0;
}

double agg_get_line_width(const Agg2D* a) {
  const BLContextState* s = a->state();
  return s ? s->strokeOptions.width : 0.0;
}

// bl:0x2378: 2 -> round, 1 -> square, else butt.
void agg_line_cap(Agg2D* a, int cap) {
  uint32_t c = cap == 2 ? 2u : uint32_t(cap == 1);
  blContextSetStrokeCaps(&a->ctx, BLStrokeCap(c));
}

// bl:0x23d0: table DAT_0017b610 = {1, 4, 3, 2} for joins 1..4, else miter clip.
void agg_line_join(Agg2D* a, int join) {
  static const uint32_t table[4] = {1, 4, 3, 2};
  uint32_t j = uint32_t(join - 1) < 4 ? table[join - 1] : 0;
  blContextSetStrokeJoin(&a->ctx, BLStrokeJoin(j));
}

// bl:0x244c.
void agg_fill_even_odd(Agg2D* a, int even_odd) {
  blContextSetFillRule(&a->ctx, BLFillRule(even_odd != 0));
}

// bl:0x2498.
void agg_reset_transformations(Agg2D* a) {
  a->transformDirty = 0;
  blContextApplyTransformOp(&a->ctx, BL_TRANSFORM_OP_RESET, nullptr);
}

// bl:0x25dc.
void agg_translate(Agg2D* a, double dx, double dy) {
  a->transformDirty = 0;
  double v[2] = {dx, dy};
  blContextApplyTransformOp(&a->ctx, BL_TRANSFORM_OP_POST_TRANSLATE, v);
}

void agg_reset_path(Agg2D* a) { blPathClear(&a->path); }
void agg_move_to(Agg2D* a, double x, double y) { blPathMoveTo(&a->path, x, y); }
void agg_line_to(Agg2D* a, double x, double y) { blPathLineTo(&a->path, x, y); }

void agg_arc_to(Agg2D* a, double rx, double ry, double angle, int large_arc, int sweep, double x, double y) {
  blPathEllipticArcTo(&a->path, rx, ry, angle, large_arc != 0, sweep != 0, x, y);
}

void agg_quadric_curve_to(Agg2D* a, double cx, double cy, double x, double y) {
  blPathQuadTo(&a->path, cx, cy, x, y);
}

void agg_cubic_curve_to(Agg2D* a, double c1x, double c1y, double c2x, double c2y, double x, double y) {
  blPathCubicTo(&a->path, c1x, c1y, c2x, c2y, x, y);
}

void agg_close_polygon(Agg2D* a) { blPathClose(&a->path); }

// bl:0x32d4 drawPath: 0 fill, 1 stroke, 2 fill then stroke, 3 fill with the line style,
// 4 stroke with the fill style, 5 stroke then fill.
void agg_draw_path(Agg2D* a, uint32_t flag) {
  switch (flag) {
    case 0: a->fillPath(&a->path, 0); break;
    case 1: a->strokePath(&a->path, 1); break;
    case 2: a->fillPath(&a->path, 0); a->strokePath(&a->path, 1); break;
    case 3: a->fillPath(&a->path, 1); break;
    case 4: a->strokePath(&a->path, 0); break;
    case 5: a->strokePath(&a->path, 1); a->fillPath(&a->path, 0); break;
    default: break;
  }
}

// bl:0x33c8: Agg2D slot 1 is the fill buffer, slot 0 the stroke buffer. Marks it dirty.
int16_t* agg_gradient_color_buffer(Agg2D* a, uint32_t slot) {
  if (slot > 1)
    return nullptr;
  int s = slot == 1 ? 0 : 1;
  a->gradientDirty[s] = 1;
  return &a->lut[s][0][0];
}

// bl:0x33f4.
void agg_linear_gradient(Agg2D* a, double x1, double y1, double x2, double y2, uint32_t slot) {
  int s = slot == 1 ? 0 : 1;
  a->style[s] = STYLE_GRADIENT;
  BLGradientCore* g = &a->gradient[s];
  blGradientSetType(g, BL_GRADIENT_TYPE_LINEAR);
  double v[4] = {x1, y1, x2, y2};
  blGradientSetValues(g, 0, v, 4);
  blGradientApplyTransformOp(g, BL_TRANSFORM_OP_ASSIGN, &a->state()->finalTransform);
}

// bl:0x3538: matrix scale(aspect, 1), post-rotate(angle), post-translate(cx, cy), post-transform
// by the context's final matrix; values (0, 0, 0, 0, r, 0).
void agg_radial_gradient_ex(Agg2D* a, double cx, double cy, double r, double angle, double aspect, uint32_t slot) {
  BLMatrix2D m;
  m.m00 = aspect;
  m.m01 = 0.0;
  m.m10 = aspect * 0.0;
  m.m11 = 1.0;
  m.m20 = aspect * 0.0;
  m.m21 = 0.0;
  double rot[1] = {angle};
  blMatrix2DApplyOp(&m, BL_TRANSFORM_OP_POST_ROTATE, rot);
  m.m20 = m.m20 + cx;
  m.m21 = m.m21 + cy;
  blMatrix2DApplyOp(&m, BL_TRANSFORM_OP_POST_TRANSFORM, &a->state()->finalTransform);
  int s = slot == 1 ? 0 : 1;
  a->style[s] = STYLE_GRADIENT;
  BLGradientCore* g = &a->gradient[s];
  blGradientSetType(g, BL_GRADIENT_TYPE_RADIAL);
  double v[6] = {0.0, 0.0, 0.0, 0.0, r, 0.0};
  blGradientSetValues(g, 0, v, 6);
  blGradientApplyTransformOp(g, BL_TRANSFORM_OP_ASSIGN, &m);
}

// bl:0x3638: the rect mapped through the inverse user matrix.
void agg_clip_box_int(Agg2D* a, int x0, int y0, int x1, int y1) {
  BLMatrix2D m = a->state()->userTransform;
  blMatrix2DInvert(&m, &m);
  BLContextImpl* impl = static_cast<BLContextImpl*>(a->ctx._d.impl);
  impl->virt->restoreClipping(impl);
  BLRect r;
  r.x = m.m20 + m.m00 * double(x0) + m.m10 * double(y0);
  r.y = m.m21 + m.m01 * double(x0) + m.m11 * double(y0);
  r.w = (m.m20 + m.m00 * double(x1) + m.m10 * double(y1)) - r.x;
  r.h = (m.m21 + m.m01 * double(x1) + m.m11 * double(y1)) - r.y;
  impl->virt->clipToRectD(impl, &r);
}

// bl:0x3828.
void agg_line_width_ex(Agg2D* a, double w, double offset) {
  double d = -1.0;
  if (0.0 <= offset)
    d = offset - w;
  agg_line_width(a, (w < 0 ? -w : w) + (d < 0 ? -d : d));
  a->customStroke = 1;
  a->csWidth = w;
  a->csOffset = d;
}

// The working path as commands and vertices (for comparing with the original library).
size_t agg_path_view(const Agg2D* a, const uint8_t** cmds, const double** verts) {
  *cmds = blPathGetCommandData(&a->path);
  *verts = reinterpret_cast<const double*>(blPathGetVertexData(&a->path));
  return blPathGetSize(&a->path);
}

} // extern "C"
