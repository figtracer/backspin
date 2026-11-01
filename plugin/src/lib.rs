//! Backspin: a port of Image-Line Gross Beat as a CLAP and VST3 plugin.
//!
//! The host mapping: the host block is split at each MIDI note
//! (decision 3), the song position is `pos_beats * 96` ticks (decision 4), offline rendering is
//! `PM_HQ_NonRealtime` (decision 5), and the transport drives play, stop and song position changes
//! (decision 6). The plugin reports 15 samples of latency.
//!
//! The audio thread runs [`Backspin::take_input`] and [`Backspin::render_block`] on its own
//! buffers: the engine, then the shape section, then Clamp, then the output guard. The guard never
//! lets a non-finite or runaway block reach the host: it outputs silence instead and resets the
//! chain. The audio thread allocates and frees nothing; the shape processors travel through
//! [`sb::Shared`] and come back as garbage.

mod decode;
mod editor;
mod params;
mod sb;
mod shared;
#[cfg(test)]
mod stress;

use backspin_engine::{
    Engine, LATENCY, NUM_PARAMS, PM_HQ_NON_REALTIME, PM_IS_RENDERING, REC_UPDATE_VALUE,
};
use nih_plug::prelude::*;
use params::BackspinParams;
use shared::{AudioQueues, Shared};
use std::sync::Arc;

/// PPQ of the song position (the plugin's own default, decision 4).
const PPQ: i32 = 96;
/// The tempo when the host gives none.
const DEFAULT_TEMPO: f64 = 120.0;
/// The output guard's limit: a sample above it (+60 dBFS) or not finite is a runaway, and the
/// block is replaced by silence. The stress tests see finite peaks near +36 dBFS from +12 dBFS
/// input at extreme settings, so this stays well clear of any signal the chain means to make.
const GUARD_LIMIT: f32 = 1024.0;
/// The most MIDI messages a block keeps (the rest are dropped, so the audio thread never grows
/// the list).
const MAX_MIDI: usize = 1024;

/// Work for nih-plug's background thread.
pub enum Task {
    /// Rebuilds the shape processor after the output guard held it out of the chain.
    RebuildShape,
}

/// The host clock of a block.
#[derive(Clone, Copy, Debug)]
struct Clock {
    /// The song position in beats at the block start, when the host gives one.
    pos: Option<f64>,
    tempo: f64,
    playing: bool,
}

/// A MIDI message at a sample offset in the block (`status | data1 << 8 | data2 << 16`).
#[derive(Clone, Copy, Debug)]
struct Midi {
    at: usize,
    msg: u32,
}

/// The plugin.
pub struct Backspin {
    params: Arc<BackspinParams>,
    shared: Arc<Shared>,
    queues: AudioQueues,
    engine: Engine,
    /// Interleaved stereo work buffer.
    buf: Vec<[f32; 2]>,
    sample_rate: f32,
    /// Host parameter values last applied to the engine. MIDI can change the engine's slot
    /// parameters, so changes are detected against the host values, not the engine values.
    host_params: [i32; NUM_PARAMS],
    tempo: f64,
    playing: bool,
    /// Song position in beats at the next block start, when the host gives one.
    next_pos: Option<f64>,
    /// Own song position in samples, for hosts without a position.
    own_pos: u64,
    /// The latency the host knows.
    latency: u32,
    /// The Clamp clipper (built in `initialize`) and its planar work buffers.
    clamp: Option<backspin_clamp::Clamp>,
    clamp_buf: [Vec<f32>; 2],
    /// The MIDI of the block, the shape section's note-ons of the block (for LFO retriggering)
    /// and the sidechain, interleaved; all preallocated in `initialize`.
    midi: Vec<Midi>,
    sb_events: Vec<backspin_shaperbox::graph::NoteEvent>,
    sidechain: Vec<[f32; 2]>,
    /// The shape processor is held out of the chain after the output guard tripped, until its
    /// replacement arrives.
    sb_hold: bool,
    /// The stage whose output tripped the guard in the last block, for the stress tests.
    #[cfg(test)]
    trip_stage: Option<&'static str>,
}

impl Default for Backspin {
    fn default() -> Self {
        let (shared, queues) = Shared::new();
        let shared = Arc::new(shared);
        Self {
            params: Arc::new(BackspinParams::new(shared.clone())),
            shared,
            queues,
            engine: Engine::new(),
            buf: Vec::new(),
            sample_rate: 44100.0,
            host_params: [0; NUM_PARAMS],
            tempo: DEFAULT_TEMPO,
            playing: false,
            next_pos: None,
            own_pos: 0,
            latency: 0,
            clamp: None,
            clamp_buf: [Vec::new(), Vec::new()],
            midi: Vec::new(),
            sb_events: Vec::new(),
            sidechain: Vec::new(),
            sb_hold: false,
            #[cfg(test)]
            trip_stage: None,
        }
    }
}

impl Backspin {
    /// Builds the audio engine in the order FL uses when it opens a plugin: sample rate, block
    /// size, time signature, tempo, process mode, state, then the parameters. Returns the latency
    /// to report.
    fn setup(&mut self, sample_rate: f32, block: usize, offline: bool) -> u32 {
        self.sample_rate = sample_rate;
        self.shared
            .sb
            .sample_rate
            .store(sample_rate.to_bits(), std::sync::atomic::Ordering::Relaxed);
        self.queues.sb.processor = self.shared.sb.build();
        self.sb_hold = false;
        let mut e = Engine::new();
        e.set_song_ticks(0.0);
        e.set_sample_rate(sample_rate.round() as i32);
        e.set_block_size(block as i32);
        e.set_time_sig(PPQ);
        e.set_tempo(self.tempo as f32);
        e.set_process_mode(if offline {
            PM_HQ_NON_REALTIME | PM_IS_RENDERING
        } else {
            0
        });
        let blob = self.shared.save_state();
        e.load_state(&blob);
        self.host_params = e.params;
        self.engine = e;
        self.apply_params();
        // Commands sent before this point are already in the model.
        while self.queues.commands.pop().is_ok() {}
        self.buf = vec![[0.0; 2]; block];
        self.clamp = Some(backspin_clamp::Clamp::new(sample_rate, block));
        self.clamp_buf = [vec![0.0; block], vec![0.0; block]];
        self.midi = Vec::with_capacity(MAX_MIDI);
        self.sb_events = Vec::with_capacity(MAX_MIDI);
        self.sidechain = vec![[0.0; 2]; block];
        self.playing = false;
        self.next_pos = None;
        self.own_pos = 0;
        self.latency = self.chain_latency();
        self.shared.status.publish(&self.engine);
        self.latency
    }

    /// Copies the host's input and sidechain into the work buffers with every sample that is
    /// not finite replaced by 0, so a NaN or infinity from the host cannot poison the state.
    /// Returns whether there is a sidechain.
    fn take_input(&mut self, l: &[f32], r: &[f32], sidechain: Option<(&[f32], &[f32])>) -> bool {
        let n = l.len();
        for ((f, &a), &b) in self.buf[..n].iter_mut().zip(l).zip(r) {
            *f = [finite_or_zero(a), finite_or_zero(b)];
        }
        if let Some((l, r)) = sidechain {
            for ((f, &a), &b) in self.sidechain[..n].iter_mut().zip(l).zip(r) {
                *f = [finite_or_zero(a), finite_or_zero(b)];
            }
        }
        sidechain.is_some()
    }

    /// Runs one block of `n` frames through the chain in place on the work buffer: the host
    /// clock and parameters, the engine split at the MIDI notes of `self.midi`, the shape section
    /// (with `sidechain`), Clamp, then the output guard. Returns true when the guard tripped: the
    /// block is silence, the engine and Clamp are reset and the shape processor waits for a
    /// rebuild ([`Task::RebuildShape`]). `self.latency` holds the latency to report afterwards.
    fn render_block(&mut self, n: usize, clock: Clock, sidechain: bool) -> bool {
        let Clock {
            pos,
            tempo,
            playing,
        } = clock;
        if tempo != self.tempo {
            self.tempo = tempo;
            self.engine.set_song_ticks(self.ticks(pos, 0));
            self.engine.set_tempo(tempo as f32);
        }
        if playing != self.playing {
            self.playing = playing;
            self.own_pos = 0;
            self.engine.set_song_ticks(self.ticks(pos, 0));
            self.engine.set_playing(playing);
        } else if let (true, Some(p), Some(want)) = (playing, pos, self.next_pos) {
            // A jump in the song position (a loop or a seek) is FPD_SongPosChanged.
            let tolerance = tempo / 60.0 / self.sample_rate as f64;
            if (p - want).abs() > tolerance {
                self.engine.set_song_ticks(self.ticks(pos, 0));
                self.engine.song_pos_changed();
            }
        }
        self.next_pos = pos.map(|p| p + n as f64 * tempo / 60.0 / self.sample_rate as f64);
        // ProcessParam reads the song position (the HOLD button schedules from it), so give it the
        // position of this block.
        self.engine.set_song_ticks(self.ticks(pos, 0));
        self.queues.apply(&mut self.engine);
        self.apply_params();

        let mut start = 0;
        self.sb_events.clear();
        for i in 0..self.midi.len() {
            let Midi { at, msg } = self.midi[i];
            let at = at.min(n);
            if msg & 0xf0 == 0x90 && (msg >> 16) & 0xff != 0 {
                self.sb_events.push(backspin_shaperbox::graph::NoteEvent {
                    offset: at.min(n.saturating_sub(1)),
                    channel: (msg & 0xf) as u8 + 1,
                    note: ((msg >> 8) & 0xff) as u8,
                    on: true,
                });
            }
            if at > start {
                self.engine.set_song_ticks(self.ticks(pos, start));
                self.engine.render(&mut self.buf[start..at]);
                start = at;
            }
            self.engine.set_song_ticks(self.ticks(pos, start));
            self.engine.midi_in(msg);
        }
        if start < n {
            self.engine.set_song_ticks(self.ticks(pos, start));
            self.engine.render(&mut self.buf[start..n]);
        }
        #[cfg(test)]
        let engine_bad = runaway(&self.buf[..n]);
        // The ShaperBox section follows the Gross Beat engine. A new processor replaces one the
        // guard held out of the chain.
        if self.queues.sb.apply() {
            self.sb_hold = false;
        }
        if self
            .shared
            .sb
            .latency_dirty
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            self.latency = self.chain_latency();
        }
        let beats = self.ticks(pos, 0) / PPQ as f64;
        if let Some(sbp) = self.queues.sb.processor.as_mut()
            && !self.sb_hold
        {
            sbp.process_with_events(
                &mut self.buf[..n],
                backspin_shaperbox::graph::Transport {
                    ppq: beats,
                    bpm: tempo,
                    playing,
                },
                sidechain.then_some(&self.sidechain[..n]),
                &self.sb_events,
            );
        }
        #[cfg(test)]
        let shape_bad = runaway(&self.buf[..n]);
        self.process_clamp(n);
        if playing {
            self.own_pos += n as u64;
        }
        self.shared.status.publish(&self.engine);
        let tripped = runaway(&self.buf[..n]);
        #[cfg(test)]
        {
            self.trip_stage = tripped.then_some(if engine_bad {
                "engine"
            } else if shape_bad {
                "shape"
            } else {
                "clamp"
            });
        }
        if tripped {
            self.buf[..n].fill([0.0; 2]);
            self.engine.flush();
            if let Some(c) = self.clamp.as_mut() {
                c.reset();
            }
            self.sb_hold = self.queues.sb.processor.is_some();
        }
        tripped
    }

    /// Clamp, the last stage: the work buffer through the clipper when it is on. Under
    /// flush-to-zero, since its filters otherwise decay into subnormals after a signal stops,
    /// which is slow on x86.
    fn process_clamp(&mut self, n: usize) {
        let p = &self.params.clamp;
        let Some(clamp) = self.clamp.as_mut() else {
            return;
        };
        let params = backspin_clamp::Params {
            bypass: !p.on.value(),
            oversampling: p.oversampling.value() as u8,
            in_db: f64::from(p.input.value()),
            out_db: f64::from(p.output.value()),
            ceiling_db: f64::from(p.ceiling.value()),
            // The original steps SHAPE by 0.01 in double.
            shape: (f64::from(p.shape.value()) * 100.0).round() / 100.0,
            kind: p.kind.value() as u8,
            mode: p.mode.value() as u8,
        };
        if params.bypass {
            return;
        }
        let [l, r] = &mut self.clamp_buf;
        let (l, r) = (&mut l[..n], &mut r[..n]);
        for (i, f) in self.buf[..n].iter().enumerate() {
            l[i] = f[0];
            r[i] = f[1];
        }
        {
            let _ftz = backspin_shaperbox::ftz::FlushToZero::new();
            clamp.process(l, r, &params);
        }
        for (i, f) in self.buf[..n].iter_mut().enumerate() {
            *f = [l[i], r[i]];
        }
        let (post, pre) = clamp.meter();
        let meter = &self.shared.status.clamp_meter;
        meter[0].store(post.to_bits(), std::sync::atomic::Ordering::Relaxed);
        meter[1].store(pre.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    /// The song position in ticks, `offset` samples after the block start.
    fn ticks(&self, pos_beats: Option<f64>, offset: usize) -> f64 {
        let beats = match pos_beats {
            Some(b) => b,
            None => self.own_pos as f64 * self.tempo / 60.0 / self.sample_rate as f64,
        };
        (beats + offset as f64 * self.tempo / 60.0 / self.sample_rate as f64) * PPQ as f64
    }

    /// Gross Beat's 15 samples, plus the shape section's when it runs (its base delay, which a
    /// state can turn off, or Drive's oversampler latency).
    fn chain_latency(&self) -> u32 {
        LATENCY as u32
            + self
                .queues
                .sb
                .processor
                .as_ref()
                .map_or(0, |p| p.latency() as u32)
    }

    /// Applies host parameter changes (FL calls ProcessParam between blocks).
    fn apply_params(&mut self) {
        for (i, p) in self.params.by_index().iter().enumerate() {
            let v = p.value();
            if v != self.host_params[i] {
                self.host_params[i] = v;
                self.engine.process_param(i, v, REC_UPDATE_VALUE);
            }
        }
    }
}

impl Plugin for Backspin {
    const NAME: &'static str = "Backspin";
    const VENDOR: &'static str = "figtracer";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            // The shape section's sidechain (Dynamics, envelope followers, audio triggers). A host
            // that leaves it unconnected gives silence (needs the vendored nih-plug's bus fix).
            aux_input_ports: &[new_nonzero_u32(2)],
            names: PortNames {
                aux_inputs: &["Sidechain"],
                ..PortNames::const_default()
            },
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = Task;

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn task_executor(&mut self) -> TaskExecutor<Self> {
        let shared = self.shared.clone();
        Box::new(move |task| match task {
            Task::RebuildShape => shared.sb.push(),
        })
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.shared.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let latency = self.setup(
            buffer_config.sample_rate,
            buffer_config.max_buffer_size as usize,
            buffer_config.process_mode == ProcessMode::Offline,
        );
        context.set_latency_samples(latency);
        true
    }

    fn reset(&mut self) {
        self.engine.flush();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let n = buffer.samples();
        if n == 0 {
            return ProcessStatus::Normal;
        }
        let transport = context.transport();
        let clock = Clock {
            pos: transport.pos_beats(),
            tempo: transport.tempo.unwrap_or(DEFAULT_TEMPO),
            playing: transport.playing,
        };
        self.midi.clear();
        while let Some(event) = context.next_event() {
            let msg = match event {
                NoteEvent::NoteOn {
                    channel,
                    note,
                    velocity,
                    ..
                } => {
                    let vel = ((velocity * 127.0).round() as u32).clamp(1, 127);
                    0x90 | channel as u32 | (note as u32) << 8 | vel << 16
                }
                NoteEvent::NoteOff { channel, note, .. } => {
                    0x80 | channel as u32 | (note as u32) << 8
                }
                _ => continue,
            };
            if self.midi.len() < self.midi.capacity() {
                self.midi.push(Midi {
                    at: event.timing() as usize,
                    msg,
                });
            }
        }
        let channels = buffer.channels();
        let sidechain = {
            let input = buffer.as_slice();
            let (l, r) = (&*input[0], &*input[channels.min(2) - 1]);
            let sc = aux.inputs.first_mut().and_then(|sc| {
                let ch = sc.as_slice();
                let (l, r) = (ch.first()?, ch.get(1).or(ch.first())?);
                Some((&l[..n], &r[..n]))
            });
            self.take_input(&l[..n], &r[..n], sc)
        };
        let latency = self.latency;
        let tripped = self.render_block(n, clock, sidechain);
        let slices = buffer.as_slice();
        for (i, f) in self.buf[..n].iter().enumerate() {
            slices[0][i] = f[0];
            if channels > 1 {
                slices[1][i] = f[1];
            }
        }
        // FL re-negotiates its delay compensation on each latency change: report real changes.
        if self.latency != latency {
            context.set_latency_samples(self.latency);
        }
        if tripped {
            context.execute_background(Task::RebuildShape);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for Backspin {
    const CLAP_ID: &'static str = "com.figtracer.backspin";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Time and volume mapping effect");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo];
}

impl Vst3Plugin for Backspin {
    const VST3_CLASS_ID: [u8; 16] = *b"BackspinFxPlugin";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx];
}

/// `x`, or 0 when it is not finite (a select, so the input loops stay branch-free).
#[inline(always)]
fn finite_or_zero(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// True when a sample of `buf` is not finite or above [`GUARD_LIMIT`]. One pass of integer
/// maxima over the magnitude bits, which the compiler vectorizes: positive floats order like
/// their bits, and NaN and infinity have the largest exponents.
fn runaway(buf: &[[f32; 2]]) -> bool {
    let mut max = 0u32;
    for f in buf {
        max = max
            .max(f[0].to_bits() & 0x7fff_ffff)
            .max(f[1].to_bits() & 0x7fff_ffff);
    }
    max > GUARD_LIMIT.to_bits()
}

nih_export_clap!(Backspin);
nih_export_vst3!(Backspin);

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard sees NaN, infinities and runaway levels in either channel, and nothing else.
    #[test]
    fn runaway_sees_bad_samples() {
        assert!(!runaway(&[
            [0.0, 0.0],
            [GUARD_LIMIT, -GUARD_LIMIT],
            [1e-40, -0.0]
        ]));
        assert!(runaway(&[[0.0, 0.0], [0.5, f32::NAN]]));
        assert!(runaway(&[[f32::INFINITY, 0.0]]));
        assert!(runaway(&[[0.0, f32::NEG_INFINITY]]));
        assert!(runaway(&[[0.0, 0.0], [-GUARD_LIMIT * 1.001, 0.0]]));
        assert!(!runaway(&[]));
    }

    /// The input sanitizer keeps finite samples, subnormals included, and zeroes the rest.
    #[test]
    fn finite_or_zero_keeps_finite_samples() {
        for x in [0.0, -0.0, 1.0, -3e38, 1e-40, f32::MAX] {
            assert_eq!(finite_or_zero(x).to_bits(), x.to_bits());
        }
        for x in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(finite_or_zero(x), 0.0);
        }
    }
}
