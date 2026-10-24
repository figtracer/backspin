#!/usr/bin/env python3
"""Test vectors for the envelope editing tools (gate 9).

The frozen reference model `.amp/in/re/data/envtools_model.py` covers the Smooth up / Filter core
without decimation and without the selection path, the kernel render and Create sequence. This
script imports it and adds, from envelope.md section 5 and the decompile, everything it lacks:
decimation (5.4), the selection path (5.10.4 G), Flip, Normalize, Scale levels, Turn all points
smooth, Prepare for smooth editing, 1 semitone up/down, Cut/Copy/Paste/Duplicate/Delete, Analyze
audio file and the value parser. It writes crates/envelope/tests/data/tools.json: each case holds
an input envelope, a tool with its arguments, and the expected points.
"""
import json
import math
import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, '..', '.amp', 'in', 're', 'data'))
import envtools_model as M  # noqa: E402
from envtools_model import Env, Pt, f32, rne, EPS24, MAXINT_LEN  # noqa: E402

LN_1001 = 6.90875477931522
LN_10001 = 9.210440366976517


def bits64(x):
    return struct.unpack('<Q', struct.pack('<d', x))[0]


def bits32(x):
    return struct.unpack('<I', struct.pack('<f', x))[0]


def sel(p):
    return p.flags & 1 != 0


def sel_idx(env):
    return [i for i, p in enumerate(env.points) if sel(p)]


def uses_selection(env):
    return len(sel_idx(env)) > 1


def continuous(env):
    s = sel_idx(env)
    return len(s) >= 2 and all(b - a <= 1 for a, b in zip(s, s[1:]))


def to_absolute(env):
    x = 0.0
    for p in env.points:
        x += p.dx
        p.dx = x


def to_delta(env):
    prev = 0.0
    for p in env.points:
        x = p.dx
        p.dx = x - prev
        prev = x


def deselect(env):
    for p in env.points:
        p.flags &= 0xfe


def delete_marked(env):
    out, carry = [], 0.0
    for p in env.points:
        if p.shape == 0xff:
            carry += p.dx
        else:
            p.dx += carry
            carry = 0.0
            out.append(p)
    env.points = out
    env.prepare()


def remove_redundant(env):
    pts = env.points
    for i in range(1, len(pts) - 1):
        y = pts[i].value
        if pts[i - 1].value == y and (pts[i + 1].value == y or (pts[i + 1].shape == 2 and pts[i].shape == 2)):
            pts[i].shape = 0xff
    delete_marked(env)


def insert_point(env, x, y, t, desel=True):
    """0x23c6b0 (fine comparison)."""
    if desel:
        deselect(env)
    x = min(x, env.length) if x >= 0.0 else 0.0
    n = len(env.points)
    idx, prev = n, 0.0
    if n >= 1:
        s = env.points[0].dx
        prev = s
        for i in range(1, n):
            s2 = s + env.points[i].dx
            if x < s2:
                idx = i
                break
            s = s2
            prev = s
    tmpl = env.points[idx] if idx < n else None
    p = Pt(0.0, y, t, 0)
    if tmpl is not None:
        p.sign, p.c = tmpl.sign, list(tmpl.c)
    d = f32(x - prev)
    p.dx = 0.0 if d <= 0.0 else d
    p.value, p.tension = f32(y), f32(t)
    env.points.insert(idx, p)
    if idx < len(env.points) - 1:
        env.points[idx + 1].dx -= env.points[idx].dx
    env.prepare()
    return idx


def find(env, x):
    s = 0.0
    for i, p in enumerate(env.points):
        s += p.dx
        if abs(s - x) < EPS24:
            return i
    return -1


def find_or_insert(env, x):
    i = find(env, x)
    if i < 0:
        i = insert_point(env, x, f32(M.get_value_at(env, x)), 0.0)
    return i


def delete_point(env, i):
    if not (1 <= i <= len(env.points) - 2):
        return
    deselect(env)
    if i < len(env.points) - 1:
        env.points[i + 1].dx += env.points[i].dx
    del env.points[i]
    env.prepare()


def fix_len_clamp(env):
    to_absolute(env)
    for p in env.points:
        if env.length <= p.dx:
            p.dx = env.length
    env.points[0].dx = 0.0
    env.points[-1].dx = env.length
    to_delta(env)


def cut_at_length(env):
    if env.length == MAXINT_LEN:
        return
    i = find_or_insert(env, env.length)
    if i < len(env.points) - 1:
        env.points = env.points[:i + 1]
        remove_redundant(env)


# ---------------------------------------------------------------- decimation (5.4, 0x239320)

def decimate(env, thr, use_sel):
    if len(env.points) < 2:
        return False
    remove_redundant(env)
    state = {'possible': False}
    dpass(env, 9.5367431640625e-07, 0.0, use_sel, state)
    if 9.5367431640625e-07 < thr:
        dpass(env, thr, 200.0, use_sel, state)
    return state['possible']


def dpass(env, thr, weight, use_sel, state):
    to_absolute(env)
    orig = [(p.dx, p.value) for p in env.points]
    n = len(env.points)
    blk = max(16, rne(math.sqrt(n)))
    sub = rne(math.sqrt(blk))
    anchor, s, end = 0, 1, n - 1
    if use_sel:
        idx = sel_idx(env)
        if len(idx) < 2:
            return
        s, end = idx[0], idx[-1]

    def limit_of(a, e):
        lim = min(e, len(env.points) - 2)
        i = a
        while i <= lim:
            x = env.points[i].dx
            nxt = env.points[i + 1]
            ok = nxt.shape in (0, 1, 7, 8, 9, 11, 12) and nxt.tension == 0.0
            if x < 0.0 or MAXINT_LEN < x or env.points[i].arp != 0 or not ok:
                return i - 1
            i += 1
        return lim

    def neg(x):
        return math.copysign(1.0, x) < 0

    def attempt(a, e, lim):
        if e > lim:
            return False
        state['possible'] = True
        pts = env.points
        dx = pts[e + 1].dx - pts[anchor].dx
        if 0.0 < dx:
            y0 = pts[anchor].value
            k = (pts[e + 1].value - y0) / dx
            for i in range(anchor + 1, e + 1):
                d = abs((y0 + k * (pts[i].dx - pts[anchor].dx)) - orig[i][1])
                if not neg(d - thr):
                    return False
                if 0.0 < weight and not neg(d * (orig[i + 1][0] - orig[i - 1][0]) * weight - thr):
                    return False
        for i in range(a, e + 1):
            pts[i].shape = 0xff
        return True

    while s < end:
        lim = limit_of(s, s + blk - 1)
        if attempt(s, s + blk - 1, lim):
            s += blk
        elif attempt(s, s + sub - 1, lim):
            s += sub
        else:
            if not attempt(s, s, lim):
                anchor = s
            s += 1
    to_delta(env)
    delete_marked(env)


def decimate_threshold(wheel):
    norm = wheel * (1.0 / 1000.0)
    return f32(f32(math.exp(f32(norm * 1.25) * LN_10001) - 1.0) / 80000.0)


# ---------------------------------------------------------------- simple tools

def flip(env):
    use = uses_selection(env)
    for p in env.points:
        if sel(p) or not use:
            p.value = f32(1.0 - p.value)
    env.prepare()


def normalize(env):
    use = uses_selection(env)
    mn, mx = 100.0, -100.0
    for p in env.points:
        if sel(p) or not use:
            mn = min(mn, p.value)
            mx = max(mx, p.value)
    if 0.0 < mx - mn:
        for p in env.points:
            if sel(p) or not use:
                p.value = f32(0.0 + (p.value - mn) * ((1.0 - 0.0) / (mx - mn)))
        env.prepare()


def scale_levels(env, center, mul, ofs, tension):
    use = uses_selection(env)
    c = center * 0.0001220703125
    r = max(1.0 - c, c)
    t = abs(f32(f32(tension) * f32(0.00012207031)))
    sg = 0.0 if tension == 0 else (1.0 if tension > 0 else -1.0)
    k = f32(f32(math.exp(t * LN_1001) - 1.0) * sg)
    kr = f32(1.0 / abs(k)) if k != 0.0 else 0.0
    kl = f32(math.log(f32(abs(k) + 1.0))) if k != 0.0 else 0.0
    for p in env.points:
        if use and not sel(p):
            continue
        d = p.value - c
        df = f32(d)
        s = 0.0 if df == 0.0 else (-1.0 if math.copysign(1.0, df) < 0 else 1.0)
        a = abs(d) * (1.0 / r)
        if k != 0.0 or math.copysign(1.0, k) < 0:
            if math.copysign(1.0, k) > 0:
                a = 1.0 - (math.exp((1.0 - a) * kl) - 1.0) * kr
            else:
                a = (math.exp(a * kl) - 1.0) * kr
        y = s * a * (mul / ((1.0 / r) * 8192.0)) + ofs * 0.0001220703125 + c
        p.value = f32(min(max(y, 0.0), 1.0))
    env.prepare()


def semitone(env, i, up):
    p = env.points[i]
    if not (0.0 < p.dx):
        return
    tag = 1.0 if up else -1.0
    y0 = env.points[max(i - 1, 0)].value
    y1 = p.value
    g = 8.0 / p.dx
    ratio = math.exp((tag / 12.0) * 0.6931471805599453)
    y = y0 + (((y1 - y0) * g + 1.0) * ratio - 1.0) / g
    p.value = f32(min(max(y, 0.0), 1.0))
    p.shape, p.tension = 0, 0.0
    env.prepare()


def decimate_tool(env, amount):
    use = uses_selection(env)
    if use and not continuous(env):
        return 'refused'
    if not decimate(env, decimate_threshold(amount), use):
        return 'refused'
    fix_len_clamp(env)
    env.prepare()
    return 'ok'


# ---------------------------------------------------------------- smooth up / filter

def raster_filter(env, att, att_slope, rel, rel_slope, ofs, kernel):
    """Sections B to E of the core: raster, padding, filter, one point per sample."""
    remove_redundant(env)
    h0 = env.raster * M.F32_0001
    length = env.abs_pos(len(env.points) - 1)
    n = rne(length / h0)
    if n + 1 <= 1:
        return None
    h = length / n
    st = M.Stepper(env)
    buf = []
    for _ in range(n + 1):
        buf.append(f32(st.out + env.offset))
        st.step(h)
    if kernel is not None:
        att = rel = float(len(kernel))
    sh = abs(rne(ofs))
    pre = max(rne(att), sh)
    post = max(rne(rel), sh)
    end_val = 0.0 if env.length == MAXINT_LEN else buf[-1]
    buf = [buf[0]] * pre + buf + [end_val] * post
    cut = pre
    if kernel is None:
        ca, cr = M.follower_coefs(att * att_slope, rel * rel_slope)
        M.follow(buf, ca, cr)
        buf.reverse()
        ca, cr = M.follower_coefs(rel * (1.0 - rel_slope), att * (1.0 - att_slope))
        M.follow(buf, ca, cr)
        buf.reverse()
    else:
        ksum = f32(math.fsum(kernel))
        if ksum == 0.0:
            kernel = [1.0] * len(kernel)
            ksum = f32(float(len(kernel)))
        inv = f32(1.0 / ksum)
        m, k = len(buf), len(kernel)
        out = []
        for j in range(m + k - 1):
            acc = 0.0
            for q in range(max(0, j - m + 1), min(k, j + 1)):
                acc += buf[j - q] * kernel[q]
            out.append(f32(f32(acc) * inv))
        buf = out
        cut += k // 2
    buf = buf[cut - rne(ofs):]
    return h, [Pt(0.0 if i == 0 else h, v) for i, v in enumerate(buf)]


def smooth_core(env, att, att_slope, rel, rel_slope, ofs, thr, kernel, use_sel):
    """0x239c90 with the selection path and the final decimation."""
    if len(env.points) < 2:
        return
    sel_end_x = -1.0
    if use_sel:
        orig = [p.copy() for p in env.points]
        to_absolute(env)
        orig_abs = [p.copy() for p in env.points]
        to_delta(env)
        idx = sel_idx(env)
        if len(idx) < 2:
            return
        last = idx[-1]
        if last < len(env.points) - 1:
            sel_end_x = env.abs_pos(last)
        env.points = env.points[:len(idx)]
        deselect(env)
        for j, i in enumerate(idx):
            p = env.points[j]
            p.dx, p.value, p.tension, p.shape = orig[i].dx, orig[i].value, orig[i].tension, orig[i].shape
            p.flags |= 1
        env.points[0].dx = 0.0
        env.prepare()
    res = raster_filter(env, att, att_slope, rel, rel_slope, ofs, kernel)
    if res is None:
        return
    h, new = res
    if not use_sel:
        env.points = new
        env.prepare()
        cut_at_length(env)
        env.prepare()
        decimate(env, thr, False)
        return
    for q in new:
        q.flags = 1
    env.points = []
    s0, last = idx[0], idx[-1]
    carry = 0.0
    for i in range(s0):
        p = Pt(0.0 if i == 0 else orig[i].dx, orig[i].value, orig[i].tension, orig[i].shape, orig[i].arp, 0)
        env.points.append(p)
        carry = p.dx
    for j, q in enumerate(new):
        p = Pt(carry if j == 0 else q.dx, q.value, 0.0, 0, 0, 1)
        env.points.append(p)
        if 0.0 <= sel_end_x and sel_end_x <= env.abs_pos(len(env.points) - 1):
            break
    if last < len(orig) - 1:
        to_absolute(env)
        for i in range(last + 1, len(orig)):
            env.points.append(Pt(orig_abs[i].dx, orig_abs[i].value, orig[i].tension, orig[i].shape, orig[i].arp, 0))
        to_delta(env)
    env.prepare()
    cut_at_length(env)
    decimate(env, thr, True)
    env.points[0].dx = 0.0


def check_against_frozen_model():
    """The whole-envelope raster and filter must equal the frozen model's core (before its
    length cut and our decimation)."""
    for pts, args in [(STEP, (250.0, f32(0.5), 250.0, f32(0.5), 0.0)), (A, (90.0, f32(0.3), 400.0, f32(0.7), -75.0))]:
        e1, e2 = mk(pts), mk(pts)
        _, want = M.smooth_core(e1, *args)
        _, new = raster_filter(e2, *args, None)
        e2.points = new
        e2.prepare()
        cut_at_length(e2)
        got = [(p.dx, p.value) for p in e2.points]
        assert got == [(p.dx, p.value) for p in want], 'raster differs from the frozen model'


def smooth_up(env, a, r, aslope, rslope, ofs, dec):
    use = uses_selection(env)
    if use and not continuous(env):
        return 'refused'
    A = f32(f32(a) * f32(0.001))
    R = f32(f32(r) * f32(0.001))
    smooth_core(env, A * A * 1000.0, f32(f32(aslope) * f32(0.001)), R * R * 1000.0,
                f32(f32(rslope) * f32(0.001)), ofs * 0.25, decimate_threshold(dec), None, use)
    fix_len_clamp(env)
    env.prepare()
    return 'ok'


def filter_tool(env, kenv, width_wheel, ofs_wheel, dec):
    use = uses_selection(env)
    if use and not continuous(env):
        return 'refused'
    w = f32(f32(width_wheel) * f32(0.001))
    width = max(w * w * 1000.0, 3.0)
    klen = (int(width) + (1 if width - int(width) > 0 else 0)) | 1
    kernel = M.render_kernel(kenv, width, klen)
    ofs = (ofs_wheel / 2000.0) * klen
    smooth_core(env, 0.0, 0.0, 0.0, 0.0, ofs, decimate_threshold(dec), kernel, use)
    fix_len_clamp(env)
    env.prepare()
    return 'ok'


# ---------------------------------------------------------------- clipboard and point tools

def copy_points(env, cut):
    s = sel_idx(env)
    if not s:
        return
    cont = all(b - a <= 1 for a, b in zip(s, s[1:]))
    if cut and (len(s) > len(env.points) - 2 or not cont):
        return
    to_absolute(env)
    base = env.points[s[0]].dx
    clip = []
    for i in s:
        q = env.points[i].copy()
        q.dx = env.points[i].dx - base
        clip.append(q)
    env.clip = clip
    to_delta(env)
    if cut:
        deselect(env)
        for i in reversed(s):
            delete_point(env, i)
        if 0 < s[0] < len(env.points):
            env.points[s[0]].dx = env.points[s[0] - 1].dx
        env.points[0].dx = 0.0
    env.prepare()


def paste_points(env, replace):
    clip = getattr(env, 'clip', [])
    if not clip:
        return 'refused'
    s = sel_idx(env)
    if replace and not continuous(env):
        return 'refused'
    if not s:
        return 'ok'
    sl = s[-1]
    to_absolute(env)
    a = [p.copy() for p in env.points]
    span = clip[-1].dx
    deselect(env)
    if not replace:
        keep, shift, base, anchor_v = sl + 1, 0.0, a[sl].dx, a[sl].value
    else:
        keep, shift, base, anchor_v = s[0], a[sl].dx - a[s[0]].dx, a[s[0]].dx, -1.0
    pts = [p.copy() for p in a[:keep]]
    for k, c in enumerate(clip):
        if k == 0 and c.dx == 0.0 and c.value == anchor_v:
            continue
        q = c.copy()
        q.dx = c.dx + base
        q.flags = c.flags | 1
        pts.append(q)
    ex, ev = pts[-1].dx, pts[-1].value
    for i in range(sl + 1, len(a)):
        if i == sl + 1 and a[i].dx + span == ex and a[i].value == ev:
            continue
        q = a[i].copy()
        q.dx = (a[i].dx + span) - shift
        q.flags = a[i].flags & 0xfe
        pts.append(q)
    env.points = pts
    to_delta(env)
    env.prepare()
    fix_len_clamp(env)
    return 'ok'


def duplicate(env):
    s = sel_idx(env)
    if len(s) < 2:
        return 'refused'
    deselect(env)
    sl = s[-1]
    a = env.abs_pos(s[0])
    b = env.abs_pos(sl)
    v_last = env.points[sl].value
    if sl < len(env.points) - 1:
        env.points[sl + 1].dx += b - a
    for k, i in enumerate(s):
        x = env.abs_pos(i)
        if k == 0 and x - a == 0.0 and env.points[s[0]].value == v_last:
            env.points[sl].flags |= 1
        else:
            p = env.points[i]
            j = insert_point(env, b + (x - a), p.value, p.tension, desel=False)
            env.points[j].shape = env.points[i].shape
            env.points[j].flags |= 1
    remove_redundant(env)
    fix_len_clamp(env)
    env.prepare()
    return 'ok'


def delete_selected(env):
    if len(env.points) <= 2:
        return 'refused'
    s = sel_idx(env)
    if s == [0]:
        return 'refused'
    for i in reversed(s):
        if env.points:
            delete_point(env, i)
    return 'ok'


def turn_smooth(env):
    use = uses_selection(env)
    if use and not continuous(env):
        return 'refused'
    s = sel_idx(env)
    a, b = (s[0], s[-1]) if use else (0, len(env.points) - 1)
    for p in env.points[a:b + 1]:
        p.shape, p.tension = 10, 0.0
    return 'ok'


def analyze(env, frames, channels, sr, tempo):
    ms = len(frames) * 1000.0 / sr
    att_ms, rel_ms, mode = (5.0, 150.0, 0) if ms >= 50.0 else (0.0, 0.0, 3)
    n = min(len(frames), 0x400000)
    dt = 2.0 / sr
    if tempo > 0.0:
        dt = tempo / (sr * 60.0)
    rel = rel_ms * sr * 0.001
    att = att_ms * sr * 0.001
    ca, cr = M.follower_coefs(att * 0.5773502691896258, rel * 0.5773502691896258)
    y = [0.0, 0.0, 0.0]
    mx = 0.0
    pts = []
    for i in range(n):
        x = frames[i][0]
        if channels > 1:
            x = f32(f32(x + frames[i][1]) / channels)
        x = f32(abs(x)) if mode & 2 == 0 else f32(f32(x + 1.0) * 0.5)
        if i == 0:
            y = [x, x, x]
        else:
            for k in range(3):
                y[k] = x + (y[k] - x) * (cr if x <= y[k] else ca)
                x = f32(y[k])
        pts.append(Pt(dt, x))
        fm = f32(mx)
        mx = x if fm <= x else fm
    env.points = pts
    if 0.0 < mx and (mode & 1 == 0 or mx > 1.0):
        sc = f32(1.0 / mx)
        for p in env.points:
            p.value = f32(p.value * sc)
    env.prepare()
    decimate(env, 0.001953125, False)
    fix_length_quirk(env)
    env.prepare()
    # assign(): a record save and load, which runs the length fix again.
    fix_length_quirk(env)
    env.prepare()


def fix_length_quirk(env):
    """0x23c260 with the point-0 quirk (4.3)."""
    if env.points and env.points[0].dx > 0.0:
        insert_point(env, 0.0, env.points[0].value, 0.0)
    M.fix_length(env)


# ---------------------------------------------------------------- cases

def mk(points, sel=()):
    env = Env([Pt(dx, y, t, s) for dx, y, t, s in points])
    for i in sel:
        env.points[i].flags |= 1
    return env


def dump(env):
    return [[bits64(p.dx), bits32(p.value), bits32(p.tension), p.shape, p.flags & 1] for p in env.points]


A = [(0.0, 0.2, 0.0, 2), (0.5, 0.8, 0.0, 2), (0.75, 0.3, 0.4, 0), (1.25, 0.9, -0.6, 1), (0.5, 0.1, 0.0, 7), (1.0, 0.6, 0.0, 0)]
LINE = [(0.0 if i == 0 else 0.0625, 0.1 + 0.0125 * i + (0.0003 if i % 7 == 3 else 0.0), 0.0, 0) for i in range(65)]
STEP = [(0.0, 0.0, 0.0, 2), (2.0, 1.0, 0.0, 2), (2.0, 1.0, 0.0, 2)]
FLAT = [(0.0, 1.0, 0.0, 2), (4.0, 1.0, 0.0, 2)]


def main():
    check_against_frozen_model()
    cases = []

    def case(name, points, tool, args, sel=(), clip=None):
        env = mk(points, sel)
        if clip is not None:
            env.clip = [p.copy() for p in clip]
        before = dump(env)
        fn = TOOLS[tool]
        res = fn(env, *args)
        cases.append({'name': name, 'points': before, 'clip': None if clip is None else [[bits64(p.dx), bits32(p.value), bits32(p.tension), p.shape, p.flags & 1] for p in clip],
                      'tool': tool, 'args': args, 'result': res or 'ok', 'want': dump(env)})

    case('flip all', A, 'flip', [])
    case('flip selection', A, 'flip', [], sel=(1, 2, 4))
    case('normalize all', A, 'normalize', [])
    case('normalize selection', A, 'normalize', [], sel=(2, 3))
    for c, m, o, t in [(0, 8192, 0, 0), (4096, 12000, -500, 3000), (8192, -8192, 1000, -6000), (2000, 16384, 0, 8192)]:
        case('scale %d %d %d %d' % (c, m, o, t), A, 'scale', [c, m, o, t])
    case('scale selection', A, 'scale', [4096, 9000, 300, -2000], sel=(1, 2, 3))
    for amt in (0, 300, 500, 1000):
        case('decimate line %d' % amt, LINE, 'decimate', [amt])
    case('decimate selection', LINE, 'decimate', [800], sel=tuple(range(10, 40)))
    case('decimate incompatible', A, 'decimate', [500])
    case('turn smooth all', A, 'smooth_all', [])
    case('turn smooth selection', A, 'smooth_all', [], sel=(2, 3, 4))
    case('turn smooth not continuous', A, 'smooth_all', [], sel=(1, 4))
    for i, up in [(1, True), (2, False), (3, True), (5, False)]:
        case('semitone %d %s' % (i, up), A, 'semitone', [i, up])
    case('smooth up step', STEP, 'smooth_up', [500, 500, 500, 500, 0, 500])
    case('smooth up step skew', STEP, 'smooth_up', [300, 800, 200, 700, 400, 250])
    case('smooth up flat', FLAT, 'smooth_up', [500, 500, 500, 500, 0, 500])
    case('smooth up A', A, 'smooth_up', [400, 600, 500, 500, -300, 600])
    case('smooth up selection', A, 'smooth_up', [400, 600, 500, 500, 0, 600], sel=(1, 2, 3))
    case('filter step', STEP, 'filter', [500, 0, 500])
    case('filter A', A, 'filter', [300, 400, 700])
    case('filter selection', A, 'filter', [200, 0, 500], sel=(2, 3, 4))
    case('copy', A, 'copy', [False], sel=(2, 3))
    case('cut', A, 'copy', [True], sel=(2, 3))
    case('cut not continuous', A, 'copy', [True], sel=(1, 3))
    clip = [Pt(0.0, 0.5, 0.0, 0, 0, 0), Pt(0.25, 0.7, 0.3, 0, 0, 1), Pt(0.5, 0.4, 0.0, 2, 0, 0)]
    case('paste insert', A, 'paste', [False], sel=(2,), clip=clip)
    case('paste replace', A, 'paste', [True], sel=(2, 3), clip=clip)
    case('paste insert at end', A, 'paste', [False], sel=(4,), clip=clip)
    case('duplicate', A, 'duplicate', [], sel=(1, 2))
    case('duplicate three', A, 'duplicate', [], sel=(2, 3, 4))
    case('delete', A, 'delete', [], sel=(2, 4))
    case('delete first only', A, 'delete', [], sel=(0,))
    seq_cases = [
        ([[1 if i < 8 else 0, 4, 65536, 32768, 32768, 32768] for i in range(32)], [0, 2, 0, 0, 500, 800, 550]),
        ([[[1, 2, 3, 0][i % 4] if i < 12 else 0, 4, 65536 - 3000 * i, 1000 * i, 30000 + 900 * i, 65536 - 1500 * i] for i in range(32)], [0, 5, 300, 120, 400, 700, 800]),
        ([[[2, 1, 3, 1, 0, 1][i % 6] if i < 10 else 0, 4, 40000 + 2000 * i, 60000, 20000, 10000] for i in range(32)], [1, 2, 600, 0, 250, 1000, 100]),
    ]
    for k, (steps, settings) in enumerate(seq_cases):
        env = mk(A)
        mode, timemul, swing, att, dec, sus, gate = settings
        e2 = M.create_sequence(steps, mode=mode, timemul=timemul, swing=swing, att=att, sus=sus, dec=dec, gate=gate)
        cases.append({'name': 'create sequence %d' % k, 'points': dump(env), 'clip': None, 'tool': 'sequence',
                      'args': [steps, settings], 'result': 'ok', 'want': dump(e2)})
    # Analyze audio: a decaying noise burst (long) and a short click.
    x = 12345

    def rnd():
        nonlocal x
        x = (x * 1103515245 + 12345) & 0x7fffffff
        return f32(x / 0x7fffffff * 2.0 - 1.0)
    long_frames = [[f32(rnd() * math.exp(-i / 3000.0)), f32(rnd() * math.exp(-i / 3000.0))] for i in range(12000)]
    short = [[f32(math.sin(i * 0.3) * 0.8), f32(math.cos(i * 0.2) * 0.5)] for i in range(1000)]
    for name, frames, ch, sr, tempo in [('analyze long', long_frames, 2, 44100.0, 0.0), ('analyze short', short, 2, 48000.0, 0.0),
                                        ('analyze tempo', long_frames, 1, 44100.0, 128.0)]:
        env = mk(A)
        before = dump(env)
        analyze(env, frames, ch, sr, tempo)
        cases.append({'name': name, 'points': before, 'clip': None, 'tool': 'analyze',
                      'args': [[[bits32(a), bits32(b)] for a, b in frames], ch, sr, tempo], 'result': 'ok', 'want': dump(env)})
    for s, pct, want in [('0.5', True, 0.5), ('50', True, 0.5), ('50%', False, 0.5), ('1/2', True, 0.5), ('0.75', False, 0.75),
                         ('3', False, 3.0), ('abc', True, None), ('2/0', True, 2.0), ('1/x', True, 1.0)]:
        cases.append({'name': 'parse %s' % s, 'tool': 'parse', 'args': [s, pct], 'want': want, 'points': [], 'clip': None, 'result': 'ok'})
    path = os.path.join(HERE, '..', 'crates', 'envelope', 'tests', 'data', 'tools.json')
    json.dump(cases, open(path, 'w'))
    print('wrote %d cases to %s' % (len(cases), os.path.normpath(path)))


TOOLS = {
    'flip': flip,
    'normalize': normalize,
    'scale': scale_levels,
    'decimate': decimate_tool,
    'smooth_all': turn_smooth,
    'semitone': semitone,
    'smooth_up': smooth_up,
    'filter': lambda env, w, o, d: filter_tool(env, kernel_env(), w, o, d),
    'copy': copy_points,
    'paste': paste_points,
    'duplicate': duplicate,
    'delete': delete_selected,
}


def kernel_env():
    """Env filter - default.fnv: (0,0), (0.5,1), (1,0), Single curve, length 1."""
    return Env([Pt(0.0, 0.0, 0.0, 0), Pt(0.5, 1.0, 0.0, 0), Pt(0.5, 0.0, 0.0, 0)], length=1.0)


if __name__ == '__main__':
    main()
