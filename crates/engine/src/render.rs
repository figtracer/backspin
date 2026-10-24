//! The render callback (Eff_Render 0x6a450, dsp.md section 1).

// The stereo loops index several per-channel arrays at once, like the original's L/R pairs.
#![allow(clippy::needless_range_loop)]

use crate::engine::Engine;
use backspin_dsp::{EPS24, fmaxnm, fmaxnm32, vforce};
use std::f64::consts::PI;

impl Engine {
    /// Renders one block in place. `buf` holds the input and receives the output.
    pub fn render(&mut self, buf: &mut [[f32; 2]]) {
        let n = buf.len();
        if n == 0 {
            return;
        }
        if !self.playing {
            self.render_stopped(buf);
            return;
        }
        self.work.reserve(n);
        self.r.reserve(n);
        self.r2.reserve(n);
        let now = self.now();
        let dnow = now as f64;
        for bank in 0..2 {
            if self.banks[bank].trigger_time as f64 <= dnow {
                self.banks[bank].trigger_time = i32::MAX;
                let pending = self.banks[bank].pending;
                self.activate(bank, pending, true);
            }
        }
        self.ui_ticks = now;
        let hold_changed = self.hold_step(now);
        if !self.holding {
            self.ring.record(buf);
        }
        self.dry.process(buf);
        let scratch_active = self.scratch < 0.0
            || self
                .scratch_sm
                .iter()
                .any(|s| EPS24 < (s.state - self.scratch as f64).abs());
        self.r.wrap_pos();
        let time = &self.banks[0];
        let time_active = (!time.active_slot().empty && self.params[2] != 0)
            || self.holding
            || scratch_active
            || self.declick[0] != 0.0
            || self.declick[1] != 0.0
            || hold_changed;
        if time_active {
            self.render_time(buf, scratch_active, hold_changed);
        } else {
            let b = &mut self.banks[0];
            let env = &b.slots[b.active_slot].env;
            b.cursor.advance(env, n as f64 * b.step);
            self.prev_env = self.two_bars;
            self.r.pos = self.r.pos.wrapping_add(n as u32);
            self.last_time_value = 0.0;
            self.xfade_pending = 0;
            self.work.wet[..n].copy_from_slice(buf);
        }
        let b = &mut self.banks[0];
        b.position = b.cursor.position(&b.slots[b.active_slot].env);
        self.render_volume(n);
        let vol = &mut self.work.vol[..n];
        if self.flush_flag {
            for s in &mut self.vol_sm {
                s.state = vol[0] as f64;
            }
        }
        for s in &mut self.vol_sm {
            s.process(vol);
        }
        for ((o, w), v) in buf.iter_mut().zip(&self.work.wet[..n]).zip(vol.iter()) {
            *o = [w[0] * v, w[1] * v];
        }
        if self.remove_dc != 0 {
            self.dc.process(buf);
        }
        self.last_out = buf[n - 1];
        self.flush_flag = false;
    }

    /// The stopped transport: the input passes through, with a one-block raised-cosine fade from
    /// the last output frame.
    fn render_stopped(&mut self, buf: &mut [[f32; 2]]) {
        if self.last_out == [0.0; 2] {
            return;
        }
        let n = buf.len();
        let den = (n as i64 - 1).max(1) as f64;
        for (i, f) in buf.iter_mut().enumerate() {
            let w = (((i as f64 * PI) / den).cos() + 1.0) * 0.5;
            let w = w as f32;
            for ch in 0..2 {
                f[ch] = f[ch] * (1.0 - w) + self.last_out[ch] * w;
            }
        }
        self.last_out = [0.0; 2];
    }

    /// The hold step (dsp.md section 7). Returns true if the hold started, looped or ended in this
    /// block with click reduction on.
    fn hold_step(&mut self, now: i32) -> bool {
        if now < self.hold_time {
            return false;
        }
        if self.hold_bits < 1 {
            if self.holding {
                self.holding = false;
                self.r.pos = self.hold_saved;
                self.hold_time = i32::MAX;
                return self.click != 0;
            }
            return false;
        }
        let len = self.sync_ticks(self.hold_index);
        if !self.holding {
            self.hold_saved = self.r.pos;
            let bar = self.spb * len as f64 / ((self.ppq as i64) << 2) as f64;
            self.hold_loop = self
                .r
                .len
                .wrapping_add(self.r.pos)
                .wrapping_sub(bar.round_ties_even() as i64 as u32);
            self.holding = true;
        }
        self.r.pos = self.hold_loop;
        self.hold_time = now.wrapping_add(len);
        self.click != 0
    }

    /// The time lane: envelope, scratch, offsets, speed, interpolation, tempo crossfade, click
    /// reduction and time mix. The result is in `work.wet`; `buf` holds the delayed dry signal.
    fn render_time(&mut self, buf: &[[f32; 2]], scratch_active: bool, hold_changed: bool) {
        let n = buf.len();
        let w = &mut self.work;
        let b = &mut self.banks[0];
        let env_buf = &mut w.env[..n];
        b.cursor
            .render_f64(&b.slots[b.active_slot].env, b.step, env_buf);
        if scratch_active {
            let s = &mut w.tmp[..n];
            s.fill(self.scratch);
            if self.flush_flag {
                for sm in &mut self.scratch_sm {
                    sm.state = s[0] as f64;
                }
            }
            for sm in &mut self.scratch_sm {
                sm.process(s);
            }
            for (e, s) in env_buf.iter_mut().zip(s.iter()) {
                *e = fmaxnm(*e + *s as f64, 0.0);
            }
        }
        self.last_time_value = env_buf[n - 1] as f32;
        if self.xfade_pending > 0 {
            w.raw[..n].copy_from_slice(env_buf);
        }
        for e in env_buf.iter_mut() {
            *e *= self.two_bars;
        }
        if self.flush_flag {
            self.prev_env = env_buf[0];
        }
        let mut taps = 8;
        if self.taps_setting < 9 {
            self.r.per_sample = false;
            if self.click != 0 {
                for (c, &e) in w.click[..n].iter_mut().zip(env_buf.iter()) {
                    *c = (e - self.prev_env) as f32;
                    self.prev_env = e;
                }
                for c in &mut w.click[..n] {
                    *c = (*c + 1.0).abs();
                }
            }
            self.prev_env = env_buf[n - 1];
        } else {
            let speed = &mut w.speed[..n];
            for (s, &e) in speed.iter_mut().zip(env_buf.iter()) {
                *s = (e - self.prev_env) as f32;
                self.prev_env = e;
            }
            for s in speed.iter_mut() {
                *s = (*s + 1.0).abs();
            }
            if self.click != 0 {
                w.click[..n].copy_from_slice(speed);
            }
            self.r.speed.process(speed);
            let (mut mn, mut mx) = (speed[0], speed[0]);
            for &s in speed.iter() {
                mn = mn.min(s);
                mx = mx.max(s);
            }
            self.r.per_sample = 1.0 < mx;
            if !self.r.per_sample {
                self.r.set_ratio(1.0);
                taps = if (1.0 - EPS24 as f32) <= mn { 8 } else { 16 };
            } else {
                taps = 16;
                for (o, &s) in w.omega[..n].iter_mut().zip(speed.iter()) {
                    *o = PI / fmaxnm32(s, 1.0) as f64;
                }
                vforce::cos(&w.omega[..n], &mut w.cos[..n]);
                for c in &mut w.cos[..n] {
                    *c *= 2.0;
                }
                self.r.omegas[..n].copy_from_slice(&w.omega[..n]);
                self.r.c2s[..n].copy_from_slice(&w.cos[..n]);
            }
        }
        if self.r.taps() != taps {
            self.r.set_taps(taps);
        }
        let wet = &mut w.wet[..n];
        self.r.read(&self.ring, env_buf, wet);
        if self.xfade_pending > 0 {
            for (o, &r) in w.off2[..n].iter_mut().zip(&w.raw[..n]) {
                *o = r * self.xfade_two_bars;
            }
            self.r2.wrap_pos();
            self.r2.per_sample = self.r.per_sample;
            if self.r.per_sample {
                self.r2.omegas[..n].copy_from_slice(&self.r.omegas[..n]);
                self.r2.c2s[..n].copy_from_slice(&self.r.c2s[..n]);
            }
            let old = &mut w.old[..n];
            self.r2.read(&self.ring, &w.off2[..n], old);
            let m = self.xfade_len.min(n as i32);
            for (i, (f, o)) in wet.iter_mut().zip(old.iter()).enumerate() {
                let t = (i as f64 / m as f64).clamp(0.0, 1.0);
                for ch in 0..2 {
                    f[ch] = (f[ch] as f64 * t + o[ch] as f64 * (1.0 - t)) as f32;
                }
            }
            self.xfade_pending = 0;
        }
        if self.click != 0 {
            let click = &mut w.click[..n];
            if hold_changed {
                click[0] = self.click_thr * 2.0;
            }
            for (f, &c) in wet.iter_mut().zip(click.iter()) {
                if self.click_thr < c {
                    for ch in 0..2 {
                        self.declick[ch] = (self.declick[ch] + self.prev_wet[ch]) - f[ch];
                    }
                }
                self.prev_wet = *f;
                for ch in 0..2 {
                    f[ch] += self.declick[ch];
                    self.declick[ch] *= self.declick_decay;
                }
            }
            for d in &mut self.declick {
                if (d.abs() as f64) < EPS24 {
                    *d = 0.0;
                }
            }
        }
        if self.params[2] != 256 {
            let m = self.params[2] as f32 * 0.00390625f32;
            let m1 = 1.0 - m;
            for (f, d) in wet.iter_mut().zip(buf) {
                for ch in 0..2 {
                    f[ch] = f[ch] * m + d[ch] * m1;
                }
            }
        }
    }

    /// The volume lane (dsp.md 6.2) into `work.vol`.
    fn render_volume(&mut self, n: usize) {
        let w = &mut self.work;
        let b = &mut self.banks[1];
        let vol = &mut w.vol[..n];
        if !b.active_slot().empty && self.params[3] != 0 {
            b.cursor
                .render_f32(&b.slots[b.active_slot].env, b.step, vol);
            let c = self.vol_curve;
            if c[0].to_bits() != 0 {
                let positive = c[0].to_bits() & 0x8000_0000 == 0;
                if positive {
                    for v in vol.iter_mut() {
                        *v = 1.0 - *v;
                    }
                }
                for v in vol.iter_mut() {
                    *v *= c[2];
                }
                let e = &mut w.expf[..n];
                vforce::expf(vol, e);
                for (v, &e) in vol.iter_mut().zip(e.iter()) {
                    *v = e + -1.0;
                }
                for v in vol.iter_mut() {
                    *v *= c[1];
                }
                if positive {
                    for v in vol.iter_mut() {
                        *v = 1.0 - *v;
                    }
                }
            }
            if self.params[3] != 256 {
                let m = self.params[3] as f32 * 0.00390625f32;
                let m1 = 1.0 - m;
                for v in vol.iter_mut() {
                    *v = *v * m + m1;
                }
            }
        } else {
            let env = &b.slots[b.active_slot].env;
            b.cursor.advance(env, n as f64 * b.step);
            vol.fill(1.0);
        }
        b.position = b.cursor.position(&b.slots[b.active_slot].env);
    }
}
