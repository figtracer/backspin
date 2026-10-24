#!/usr/bin/env python3
"""Test vectors for the 13 envelope shapes, from the formulas in envelope.md section 2.

Writes crates/envelope/tests/data/shapes.txt. Each line describes one evaluation:
    shape tension_bits(f32) x_bits(f64) weight_bits(f64)
for a 2-point envelope (0, 0) -> (1, 1) whose segment has the given shape and tension, and a
Smooth block for envelopes with Smooth neighbours. The model is written from the note, not from
the Rust port, so the test checks the port against an independent reading of the spec.
"""
import math, struct, os

def f32(x):
    return struct.unpack('<f', struct.pack('<f', x))[0]

def bits32(x):
    return struct.unpack('<I', struct.pack('<f', x))[0]

def bits64(x):
    return struct.unpack('<Q', struct.pack('<d', x))[0]

def rne(x):
    return int(round(x))  # Python round() is half to even

LN1M = 13.815511557963774
EPS = 5.960464477539063e-08

def step_count(shape, t):
    if not 3 <= shape <= 6:
        return -1
    a = f32(1.0 - abs(t) * 0.6)
    e = f32(math.exp(a * LN1M) - 1.0)
    n = rne(e * 0.001)
    if shape == 3 and t >= 0.0:
        n += 1
    return n

def curve_params(k):
    c = [k, 0.0, 0.0]
    if k != 0.0:
        c[1] = f32(1.0 / abs(k))
        c[2] = f32(math.log(f32(abs(k) + 1.0)))
    return c

def prepare(pts, i):
    """pts: list of dicts with dx, y (f32), t (f32), shape. Returns (sign, c)."""
    p = pts[i]
    t = p['t']
    sign = 0 if t == 0.0 else (-1 if math.copysign(1.0, t) < 0 else 1)
    n = step_count(p['shape'], t)
    c = [0.0, 0.0, 0.0]
    s = p['shape']
    if s in (0, 1):
        sg = 0.0 if abs(t) == 0.0 else float(sign)
        c = curve_params(f32(f32(math.exp(abs(t) * LN1M) - 1.0) * sg))
    elif s == 3:
        sv = -1.0 if t < 0.0 else 1.0
        c[2] = f32(sv * 0.5); c[0] = f32(n + 1); c[1] = f32(1.0 / f32(c[0] - sv))
    elif s == 4:
        c[0] = f32(n + 1); c[1] = f32(1.0 / c[0])
    elif s == 5:
        sv = -1.0 if t < 0.0 else 1.0
        c[2] = f32(sv * 0.5); c[0] = f32((n + 1) * 2); c[1] = f32(2 * math.pi)
    elif s == 6:
        c[0] = f32(n + 0.5); c[1] = f32(2 * math.pi)
        if n == 0 and sign > 0:
            sign = 2
    elif s in (7, 8):
        c[0] = t
    elif s == 9:
        c[0] = abs(t)
    elif s == 10:
        length = p['dx'] if p['dx'] > EPS else EPS
        y1 = p['y']; y0 = pts[i - 1]['y']; ym = y0; r0 = 1.0
        if pts[i - 1]['shape'] == 10:
            r0 = pts[i - 1]['dx'] / length
            if i >= 2:
                ym = pts[i - 2]['y']
        if i + 1 < len(pts) and pts[i + 1]['shape'] == 10:
            r1 = pts[i + 1]['dx'] / length; yn = pts[i + 1]['y']
        else:
            r1 = 1.0; yn = y1
        dy = y1 - y0
        if not abs(dy) >= EPS:
            return 2, c
        q0 = max(r0, EPS); q1 = max(r1, EPS)
        a = f32(((yn - y0) / dy - 1.0) / q1)
        if a > 0.0:
            P = f32(f32(f32(q1) * 2.0) + 1.0); Q = f32(f32(q1) + 2.0)
            a = f32(f32(a * f32(P + Q)) / f32(f32(P * a) + Q))
        else:
            a = 0.0
        b = f32((-(ym - y0) / dy) / q0)
        if b > 0.0:
            P = f32(f32(f32(q0) * 2.0) + 1.0); Q = f32(f32(q0) + 2.0)
            b = f32(f32(b * f32(Q + P)) / f32(Q + f32(P * b)))
        else:
            b = 0.0
        c = [a, b, f32(1.0 - abs(t))]
    elif s in (11, 12):
        d = 1.0 - max(abs(t) * 0.9, 1.52587890625e-05)
        c0 = f32(1.0 - d * d * d)
        c1 = f32(f32(1.0 - c0) / c0)
        c2 = c1
        if sign > 0:
            c1 = -c1; c2 = f32(c1 - 1.0)
        if s == 12:
            c1 = f32(c1 * 0.5); c2 = f32(c2 * 0.5)
        c = [c0, c1, c2]
    if s in (0, 1, 7, 8, 9, 11, 12) and t == 0.0:
        sign = 2
    return sign, c

def single(c, x):
    k = c[0]
    if k == 0.0 and math.copysign(1.0, k) > 0:
        return x
    r, l = c[1], c[2]
    if math.copysign(1.0, k) > 0:
        return 1.0 - (math.exp((1.0 - x) * l) - 1.0) * r
    return (math.exp(x * l) - 1.0) * r

def ev(shape, sign, c, x):
    if sign == 2:
        return x
    c0, c1, c2 = c
    if shape == 0:
        return single(c, x)
    if shape == 1:
        if x < 0.5:
            return single(c, 2.0 * x) * 0.5
        return 1.0 - single(c, (1.0 - x) * 2.0) * 0.5
    if shape == 3:
        return f32(f32(rne(x * c0 - c2)) * c1)
    if shape == 4:
        u = x * c0 - 0.5
        fl = float(rne(u - 0.5))
        v = (u - fl) * 2.0 - 1.0
        if sign < 0:
            q = 1.0 - abs(v)
            w = (1.0 - q * q * q) * math.copysign(1.0, v)
        else:
            w = v * v * v
        return (fl + w * 0.5 + 1.0) * c1
    if shape == 5:
        return float(rne(x * c0 - c2) & 1)
    if shape == 6:
        if sign < 0:
            return (math.sin((x * c0 + 0.75) * c1) + 1.0) * 0.5
        u = x * c0 - 0.5
        return abs((u - float(rne(u - 0.5))) * 2.0 - 1.0)
    if shape == 7:
        d = 1.0 - x if sign > 0 else x
        d3 = d * d * d
        return x - (d3 * d3 - d) * c0
    if shape == 8:
        if x < 0.5:
            y = 2.0 * x; d = 1.0 - y if sign > 0 else y; d3 = d * d * d
            return (y - (d3 * d3 - d) * c0) * 0.5
        y = (1.0 - x) * 2.0; d = 1.0 - y if sign > 0 else y; d3 = d * d * d
        return 1.0 - (y - (d3 * d3 - d) * c0) * 0.5
    if shape == 9:
        hp = f32(math.pi / 2)
        h = 1.0 - math.cos(x * hp) if sign < 1 else math.sin(x * hp)
        return x - (x - h) * c0
    if shape == 10:
        x2 = x * x; xm = x - 1.0
        return x + (((x2 * 3.0 - x2 * 2.0 * x) + x2 * xm * c0 + x * xm * xm * c1) - x) * c2
    if shape == 11:
        if sign < 1:
            return c1 / (1.0 - c0 * x) - c2
        return c1 / (1.0 - c0 * (1.0 - x)) - c2
    if shape == 12:
        if x < 0.5:
            if sign < 1:
                return c1 / (1.0 - c0 * x * 2.0) - c2
            return c1 / (1.0 - c0 * (1.0 - x * 2.0)) - c2
        u = (1.0 - x) * 2.0
        if sign < 1:
            return (1.0 - c1 / (1.0 - c0 * u)) + c2
        return (1.0 - c1 / (1.0 - c0 * (1.0 - u))) + c2
    return 0.0

TENSIONS = [-1.0, -0.9, -0.735, -0.5, -0.25, -0.1, -0.01, 0.0, 0.01, 0.1, 0.25, 0.5, 0.625, 0.735, 0.9, 1.0]
XS = [i / 64.0 for i in range(64)] + [0.3333333333333333, 0.4999999999, 0.5000000001, 0.999999]

def main():
    out = []
    for shape in range(14):
        for t in TENSIONS:
            t = f32(t)
            pts = [dict(dx=0.0, y=0.0, t=0.0, shape=2), dict(dx=1.0, y=1.0, t=t, shape=shape)]
            sign, c = prepare(pts, 1)
            for x in XS:
                out.append('%d %08x %016x %016x' % (shape, bits32(t), bits64(x), bits64(ev(shape, sign, c, x))))
    # Smooth segments with Smooth neighbours: points (dx, y), all Smooth, tension t on the middle.
    for t in TENSIONS[::3]:
        t = f32(t)
        ys = [0.1, 0.8, 0.3, 0.9, 0.9]
        dxs = [0.0, 0.5, 1.25, 0.75, 1.5]
        pts = [dict(dx=d, y=f32(y), t=t, shape=10) for d, y in zip(dxs, ys)]
        for i in range(1, len(pts)):
            sign, c = prepare(pts, i)
            for x in XS[::4]:
                out.append('smooth%d %08x %016x %016x' % (i, bits32(t), bits64(x), bits64(ev(10, sign, c, x))))
    path = os.path.join(os.path.dirname(__file__), '..', 'crates', 'envelope', 'tests', 'data', 'shapes.txt')
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, 'w').write('\n'.join(out) + '\n')
    print('wrote %d vectors to %s' % (len(out), os.path.normpath(path)))

if __name__ == '__main__':
    main()
