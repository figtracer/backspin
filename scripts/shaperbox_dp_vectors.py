#!/usr/bin/env python3
"""Generates the Dynamics / envelope follower / Pitch reference vectors for
crates/shaperbox/tests/shapers_dp.rs from the exact Python models in .amp/in/sb/data (the
reverse-engineering workspace, not in the repository). Run with `python3 -I`."""
import math
import os
import struct
import sys
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, ".amp/in/sb/data")
OUT = os.path.join(ROOT, "crates/shaperbox/tests/data")
sys.path.insert(0, DATA)
os.environ.setdefault("SB_CACHE", os.path.join(os.path.dirname(os.path.abspath(__file__)), "cache"))

import sb_dynamics_exact as D  # noqa: E402
import sb_envfollow_exact as E  # noqa: E402
import sb_pitch_exact as P  # noqa: E402

_F = struct.Struct("<f")


def r32(x):
    y = _F.unpack(_F.pack(x))[0]
    return 0.0 if -1.1754943508222875e-38 < y < 1.1754943508222875e-38 else y


# ---- signals: f32 basic ops only, mirrored in the Rust test ------------------------------------

def noise(n, seed, amp):
    x = seed
    out = []
    for i in range(n):
        x = (x * 1103515245 + 12345) & 0x7FFFFFFF
        u = r32(r32(float(x)) * 2.0 ** -31)
        v = r32(r32(u * 2.0) - 1.0)
        out.append(r32(v * amp(i)))
    return out


def osc(n, c, y1, start=0, period=None, decay=None):
    c2 = r32(2.0 * r32(c))
    out = [0.0] * n
    ya, yb, env = 0.0, r32(y1), 1.0
    for i in range(start, n):
        if period is not None and (i - start) % period == 0:
            ya, yb, env = 0.0, r32(y1), 1.0
        y = r32(r32(c2 * yb) - ya)
        ya, yb = yb, y
        if decay is not None:
            env = r32(env * r32(decay))
            out[i] = r32(y * env)
        else:
            out[i] = y
    return out


def bursts(n, seed):
    return noise(n, seed, lambda i: 0.8 if (i // 300) % 2 == 0 else 0.02)


def drums(n):
    tone = osc(n, 0.9986, 0.05, start=50, period=700, decay=0.9985)
    nz = noise(n, 7, lambda i: 0.5)
    out = []
    for i in range(n):
        k = (i - 50) % 700 if i >= 50 else 10 ** 9
        out.append(r32(tone[i] + nz[i]) if k < 64 else tone[i])
    return out


def sine(n):
    return osc(n, 0.99144, 0.065, start=200)


def harmonic(n):
    a = osc(n, 0.99979, 0.004)
    b = osc(n, 0.99916, 0.006)
    c = osc(n, 0.99811, 0.005)
    return [r32(r32(a[i] + b[i]) + c[i]) for i in range(n)]


def clicks(n):
    x = [0.0] * n
    for s in range(300, n, 1500):
        for k in range(3):
            if s + k < n:
                x[s + k] = r32(0.8 * (1.0 - k / 3.0) * (1 if k % 2 == 0 else -1))
    return x


def signal(name, n):
    if name == "bursts":
        return bursts(n, 1), bursts(n, 2)
    if name == "drums":
        d = drums(n)
        return d, d
    if name == "sine":
        s = sine(n)
        return s, s
    if name == "harmonic":
        h = harmonic(n)
        return h, [r32(v * 0.75) for v in h]
    if name == "clicks":
        c = clicks(n)
        return c, c
    if name == "noise":
        return noise(n, 11, lambda i: 0.5), noise(n, 12, lambda i: 0.5)
    raise ValueError(name)


STRIDE = 1
NCH = 2
OUT_FULL = None


def pack(path, cases, stride=None, nch=None):
    """Per case: name, channel count, stride, total length, then every stride-th value of each kept
    channel. OUT_FULL set: every sample of every channel into that directory instead."""
    if OUT_FULL is not None:
        path = os.path.join(OUT_FULL, os.path.basename(path))
        stride, nch = 1, 9
    out = bytearray(struct.pack("<I", len(cases)))
    for name, chans in cases:
        chans = chans[:nch]
        nb = name.encode()
        out += struct.pack("<B", len(nb)) + nb + struct.pack("<BBI", len(chans), stride, len(chans[0]))
        for ch in chans:
            v = ch[::stride]
            out += struct.pack("<%df" % len(v), *v)
    data = zlib.compress(bytes(out), 9)
    open(path, "wb").write(data)
    print("%s: %d cases, %d bytes" % (os.path.basename(path), len(cases), len(data)))


SR = 48000

# ---- Dynamics ---------------------------------------------------------------------------------

DYN_BASE = dict(active=True, mode=0, useSidechain=False, filterSetupIsFree=False, hpCutoff=0.0,
                lpCutoff=1.0, threshold=0.5, amount=0.0, depth=0, attack=0.0, hold=0.0, release=0.0,
                autoRelease=False, knee=0.0, offset=0.5, makeup=0.5, autoMakeup=False)

# name, input, sidechain, overrides (mirrored in the Rust test)
DYN_CASES = [
    ("sine defaults", "sine", None, {}),
    ("bursts attack .3 release .5", "bursts", None, dict(attack=0.3, release=0.5)),
    ("drums hold .3 autoRelease", "drums", None, dict(attack=0.05, release=0.3, hold=0.3, autoRelease=True)),
    ("sine thr .8 amount .9 makeup .8", "sine", None, dict(threshold=0.8, amount=0.9, makeup=0.8)),
    ("drums autoMakeup", "drums", None, dict(autoMakeup=True, amount=0.3, attack=0.4, makeup=0.6)),
    ("bursts inert knee offset depth mode", "bursts", None,
     dict(knee=1.0, offset=0.0, depth=2, mode=1, attack=0.1, release=0.3)),
    ("drums free hp .5 lp .7", "drums", None, dict(attack=0.1, release=0.3, filterSetupIsFree=True, hpCutoff=0.5, lpCutoff=0.7)),
    ("bursts sidechain drums", "bursts", "drums", dict(attack=0.1, release=0.3, useSidechain=True, autoMakeup=True)),
    ("bursts sidechain without bus", "bursts", "none", dict(attack=0.1, release=0.3, useSidechain=True)),
    ("bursts hold .2", "bursts", None, dict(attack=0.1, release=0.3, hold=0.2)),
    ("bursts free hp .3", "bursts", None, dict(attack=0.2, release=0.1, filterSetupIsFree=True, hpCutoff=0.3)),
    ("bursts free sidechain drums", "bursts", "drums", dict(attack=0.1, release=0.3, useSidechain=True, filterSetupIsFree=True, lpCutoff=0.6)),
]


def gen_dynamics(n=2400):
    cases = []
    for name, sig, sc, ov in DYN_CASES:
        xl, xr = signal(sig, n)
        p = dict(DYN_BASE)
        p.update(ov)
        band = D.DynamicsBand(SR, p)
        if sc == "none":
            scl = scr = None
        elif sc is not None:
            scl, scr = signal(sc, n)
        else:
            scl = scr = None
        dl, dr = band.detector_input(xl, xr, xl, xr, scl, scr)
        _, _, gr = band.process(xl, xr, dl, dr)
        cases.append((name, [gr]))
    pack(os.path.join(OUT, "dynamics.bin"), cases, 2, 1)


# ---- section envelope follower ----------------------------------------------------------------

EF_BASE = dict(active=True, mode=0, useSidechain=False, filterSetupIsFree=False, hpCutoff=0.0, lpCutoff=1.0,
               threshold=0.0, amount=0.5, depth=0, attack=0.1, hold=0.0, release=0.2, autoRelease=False,
               knee=0.5, offset=0.5, makeup=0.0, autoMakeup=False)

# name, input, shaper kind, port w, overrides
EF_CASES = [
    ("mode 0 defaults", "bursts", "default", 0.5, {}),
    ("mode 0 amount .8 depth 1 offset .3", "bursts", "default", 0.5, dict(amount=0.8, depth=1, offset=0.3)),
    ("mode 1 amount .3 depth 2", "drums", "default", 0.25, dict(mode=1, amount=0.3, depth=2)),
    ("mode 0 threshold .3 hold .4", "drums", "default", 0.5, dict(threshold=0.3, hold=0.4, amount=0.9)),
    ("mode 0 autoRelease", "drums", "default", 0.5, dict(autoRelease=True, attack=0.3, release=0.5, amount=0.9)),
    ("mode 1 autoRelease hold .3", "bursts", "default", 0.65, dict(mode=1, autoRelease=True, hold=0.3, release=0.1, amount=0.7)),
    ("mode 0 free hp .4 lp .8", "drums", "default", 0.5, dict(filterSetupIsFree=True, hpCutoff=0.4, lpCutoff=0.8, amount=0.9)),
    ("reverb mode 0", "bursts", "reverb", 0.8, dict(amount=0.3, depth=1)),
    ("pitch mode 1", "drums", "pitch", 0.7, dict(mode=1, amount=0.6, offset=0.6)),
    ("noise depth 2", "bursts", "noise", 0.4, dict(amount=0.9, depth=2)),
]


def gen_envfollow(n=2400):
    cases = []
    for name, sig, kind, w, ov in EF_CASES:
        xl, xr = signal(sig, n)
        p = dict(EF_BASE)
        p.update(ov)
        sf = E.SectionFollower(SR, p, kind)
        det = sf.filt.process(xl, xr) if sf.free else (xl, xr)
        port, _ = sf.process([w] * n, det[0], det[1])
        cases.append((name, [port]))
    pack(os.path.join(OUT, "envfollow.bin"), cases, 2, 1)


# ---- Pitch ------------------------------------------------------------------------------------

# name, mode, input, pitch w, formant w, link, fill, grain
PITCH_CASES = [
    ("cyclic noise ratio 2 gc .5", 3, "noise", 0.75, 0.5, True, 1.0, 0.5),
    ("cyclic noise ratio .5 gc .25", 3, "noise", 0.25, 0.5, True, 1.0, 0.25),
    ("cyclic sine ratio ramp", 3, "sine", "ramp", 0.5, True, 1.0, 0.1),
    ("psola vocals noise ratio 1", 1, "noise", 0.5, 0.5, True, 1.0, 0.5),
    ("psola vocals noise ratio 2", 1, "noise", 0.75, 0.5, True, 1.0, 0.5),
    ("psola vocals noise ratio .5 fill 0", 1, "noise", 0.25, 0.5, True, 0.0, 0.5),
    ("psola vocals noise ratio 2 formant unlinked", 1, "noise", 0.75, 0.5, False, 1.0, 0.5),
    ("psola vocals noise ratio .5 formant 2 unlinked", 1, "noise", 0.25, 0.75, False, 0.5, 0.5),
    ("psola vocals sine ratio 2 (detector)", 1, "sine", 0.75, 0.5, True, 1.0, 0.5),
    ("psola vocals harmonic ratio .5 formant unlinked (detector)", 1, "harmonic", 0.25, 0.625, False, 1.0, 0.5),
    ("psola drums noise ratio 1", 2, "noise", 0.5, 0.5, True, 1.0, 0.5),
    ("psola drums clicks ratio 2", 2, "clicks", 0.75, 0.5, True, 1.0, 0.5),
    ("psola drums clicks ratio .5 unlinked", 2, "clicks", 0.25, 0.5, False, 1.0, 0.5),
    ("psola drums drums ratio 2 (detector)", 2, "drums", 0.75, 0.5, True, 1.0, 0.5),
]


# name, input, pitch w ("ramp": the ramp below), grainSizeWsola (mode 0; mirrored in the Rust test)
WSOLA_CASES = [
    ("wsola noise ratio 2 grain 0", "noise", 0.75, 0),
    ("wsola noise ratio .5 grain 2", "noise", 0.25, 2),
    ("wsola sine ratio sqrt2 grain 1", "sine", 0.625, 1),
    ("wsola drums ratio 2 (transients)", "drums", 0.75, 0),
    ("wsola clicks ratio 1", "clicks", 0.5, 0),
    ("wsola clicks ratio .5 grain 1", "clicks", 0.25, 1),
    ("wsola sine ratio ramp", "sine", "ramp", 0),
]


def ramp(n):
    return [r32(r32(0.25) + r32(r32(float(i)) * r32(1.0 / 9600.0))) for i in range(n)]


def gen_pitch(n=4800):
    cases = []
    for name, mode, sig, wp, wf, link, fill, gc in PITCH_CASES:
        xl, xr = signal(sig, n)
        sh = P.PitchShaper(SR, mode, link_formant=link, fill_gaps=fill)
        wpv = ramp(n) if wp == "ramp" else [r32(wp)] * n
        ratio = [P.ratio_from_wave(w) for w in wpv]
        formant = [P.ratio_from_wave(r32(wf))] * n
        ol, orr = [], []
        for b in range(0, n, 512):
            e = min(n, b + 512)
            o1, o2 = sh.block(xl[b:e], xr[b:e], ratio[b:e], formant[b:e], [r32(gc)] * (e - b))
            ol += o1
            orr += o2
        cases.append((name, [ol, orr]))
        print("  pitch", name, flush=True)
    nw = 2 * n
    for name, sig, wp, gw in WSOLA_CASES:
        xl, xr = signal(sig, nw)
        sh = P.PitchShaper(SR, 0, grain_wsola=gw)
        wpv = ramp(nw) if wp == "ramp" else [r32(wp)] * nw
        ratio = [P.ratio_from_wave(w) for w in wpv]
        ol, orr = [], []
        for b in range(0, nw, 512):
            e = min(nw, b + 512)
            o1, o2 = sh.block(xl[b:e], xr[b:e], ratio[b:e])
            ol += o1
            orr += o2
        cases.append((name, [ol, orr]))
        print("  pitch", name, flush=True)
    pack(os.path.join(OUT, "pitch.bin"), cases, 3, 1)


if __name__ == "__main__":
    args = sys.argv[1:]
    if args and args[0].startswith("--full="):
        OUT_FULL = args.pop(0)[7:]
        os.makedirs(OUT_FULL, exist_ok=True)
    which = args or ["dynamics", "envfollow", "pitch"]
    if not D.EXACT:
        print("warning: clang helper unavailable; Dynamics vectors are not exact")
    for w in which:
        globals()["gen_" + w]()
