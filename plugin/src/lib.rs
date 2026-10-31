//! Backspin: a port of Image-Line Gross Beat as a CLAP and VST3 plugin.
//!
//! The host mapping follows SPEC.md section 4: the host block is split at each MIDI note
//! (decision 3), the song position is `pos_beats * 96` ticks (decision 4), offline rendering is
//! `PM_HQ_NonRealtime` (decision 5), and the transport drives play, stop and song position changes
//! (decision 6). The plugin reports 15 samples of latency.

mod decode;
mod editor;
mod params;
mod sb;
mod shared;

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
    /// The Clamp clipper (built in `initialize`) and its planar work buffers.
    clamp: Option<backspin_clamp::Clamp>,
    clamp_buf: [Vec<f32>; 2],
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
            clamp: None,
            clamp_buf: [Vec::new(), Vec::new()],
        }
    }
}

impl Backspin {
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

    /// Gross Beat's 15 samples, plus the ShaperBox base delay when that section runs.
    fn latency(&self) -> u32 {
        LATENCY as u32
            + if self.queues.sb.processor.is_some() {
                backspin_shaperbox::graph::BASE_DELAY as u32
            } else {
                0
            }
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
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.shared.clone())
    }

    /// Builds the audio engine in the order FL uses when it opens a plugin: sample rate, block
    /// size, time signature, tempo, process mode, state, then the parameters.
    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.shared.sb.sample_rate.store(
            buffer_config.sample_rate.to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
        self.queues.sb.processor = self.shared.sb.build();
        context.set_latency_samples(self.latency());
        let block = buffer_config.max_buffer_size as usize;
        let mut e = Engine::new();
        e.set_song_ticks(0.0);
        e.set_sample_rate(buffer_config.sample_rate.round() as i32);
        e.set_block_size(block as i32);
        e.set_time_sig(PPQ);
        e.set_tempo(self.tempo as f32);
        let offline = buffer_config.process_mode == ProcessMode::Offline;
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
        self.clamp = Some(backspin_clamp::Clamp::new(buffer_config.sample_rate, block));
        self.clamp_buf = [vec![0.0; block], vec![0.0; block]];
        self.playing = false;
        self.next_pos = None;
        self.own_pos = 0;
        self.shared.status.publish(&self.engine);
        true
    }

    fn reset(&mut self) {
        self.engine.flush();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let n = buffer.samples();
        if n == 0 {
            return ProcessStatus::Normal;
        }
        let transport = context.transport();
        let pos = transport.pos_beats();
        let tempo = transport.tempo.unwrap_or(DEFAULT_TEMPO);
        if tempo != self.tempo {
            self.tempo = tempo;
            self.engine.set_song_ticks(self.ticks(pos, 0));
            self.engine.set_tempo(tempo as f32);
        }
        let playing = transport.playing;
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

        let channels = buffer.channels();
        {
            let slices = buffer.as_slice();
            for (i, f) in self.buf[..n].iter_mut().enumerate() {
                let l = slices[0][i];
                *f = [l, if channels > 1 { slices[1][i] } else { l }];
            }
        }
        let mut start = 0;
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
            let at = (event.timing() as usize).min(n);
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
        // The ShaperBox section follows the Gross Beat engine.
        self.queues.sb.apply();
        if self
            .shared
            .sb
            .latency_dirty
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            context.set_latency_samples(self.latency());
        }
        let beats = self.ticks(pos, 0) / PPQ as f64;
        if let Some(sbp) = self.queues.sb.processor.as_mut() {
            sbp.process(
                &mut self.buf[..n],
                backspin_shaperbox::graph::Transport {
                    ppq: beats,
                    bpm: tempo,
                    playing,
                },
            );
        }
        self.process_clamp(n);
        let slices = buffer.as_slice();
        for (i, f) in self.buf[..n].iter().enumerate() {
            slices[0][i] = f[0];
            if channels > 1 {
                slices[1][i] = f[1];
            }
        }
        if playing {
            self.own_pos += n as u64;
        }
        self.shared.status.publish(&self.engine);
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

nih_export_clap!(Backspin);
nih_export_vst3!(Backspin);
