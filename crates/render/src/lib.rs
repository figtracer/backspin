//! Offline rendering through the engine with the flags and call order of the Gross Beat harness
//! (`.amp/in/harness/src/main.rs`), so the two can be compared sample by sample.

pub mod wav;

use backspin_engine::{
    Engine, NUM_PARAMS, PARAMS, REC_GET_VALUE, REC_UPDATE_CONTROL, REC_UPDATE_VALUE, param_text,
};

/// A note time: samples, seconds or beats from the render start.
#[derive(Clone, Copy, Debug)]
pub enum TimeSpec {
    Samples(u64),
    Seconds(f64),
    Beats(f64),
}

/// A MIDI note event.
#[derive(Clone, Copy, Debug)]
pub struct Note {
    pub at: TimeSpec,
    pub note: u8,
    pub on: bool,
}

/// The command line.
#[derive(Clone, Debug)]
pub struct Args {
    pub input: Option<String>,
    pub generator: Option<String>,
    pub seconds: f64,
    pub output: Option<String>,
    pub dump_input: Option<String>,
    pub tempo: f64,
    pub sr: u32,
    pub block: usize,
    pub ppq: i32,
    pub pos_beats: f64,
    pub state: Option<String>,
    pub save_state: Option<String>,
    pub params: Vec<(i32, i32)>,
    pub param_at: Vec<(TimeSpec, i32, i32)>,
    pub notes: Vec<Note>,
    pub process_mode: Option<i32>,
    pub list_params: bool,
}

pub const USAGE: &str = "usage: backspin-render [--input in.wav | --gen impulse:PERIOD|sine:HZ|ramp[:PERIOD] --seconds S]
               [--output out.wav] [--dump-input in.wav] [--tempo 120] [--sr 48000] [--block 512]
               [--ppq 96] [--pos BEATS] [--state state.bin|preset.fst] [--param idx=value ...]
               [--note TIME:NOTE:on|off ...] [--param-at TIME:idx=value ...] [--process-mode FLAGS] [--list-params]
               [--save-state state.bin]
TIME is samples (\"24000\"), seconds (\"0.5s\") or beats (\"2b\") from the render start.";

impl Args {
    /// Parses the flags (the harness flags plus `--save-state`).
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut a = Self {
            input: None,
            generator: None,
            seconds: 4.0,
            output: None,
            dump_input: None,
            tempo: 120.0,
            sr: 48000,
            block: 512,
            ppq: 96,
            pos_beats: 0.0,
            state: None,
            save_state: None,
            params: Vec::new(),
            param_at: Vec::new(),
            notes: Vec::new(),
            process_mode: None,
            list_params: false,
        };
        let mut it = args.into_iter();
        while let Some(flag) = it.next() {
            let mut val = || it.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--input" => a.input = Some(val()?),
                "--gen" => a.generator = Some(val()?),
                "--seconds" => a.seconds = num(&val()?)?,
                "--output" => a.output = Some(val()?),
                "--dump-input" => a.dump_input = Some(val()?),
                "--tempo" => a.tempo = num(&val()?)?,
                "--sr" => a.sr = num(&val()?)?,
                "--block" => a.block = num(&val()?)?,
                "--ppq" => a.ppq = num(&val()?)?,
                "--pos" => a.pos_beats = num(&val()?)?,
                "--state" => a.state = Some(val()?),
                "--save-state" => a.save_state = Some(val()?),
                "--process-mode" => a.process_mode = Some(num(&val()?)?),
                "--param" => {
                    let v = val()?;
                    let (i, x) = v.split_once('=').ok_or("--param wants idx=value")?;
                    a.params.push((num(i)?, num(x)?));
                }
                "--param-at" => {
                    let v = val()?;
                    let (t, rest) = v.split_once(':').ok_or("--param-at wants TIME:idx=value")?;
                    let (i, x) = rest
                        .split_once('=')
                        .ok_or("--param-at wants TIME:idx=value")?;
                    a.param_at.push((time_spec(t)?, num(i)?, num(x)?));
                }
                "--note" => {
                    let v = val()?;
                    let parts: Vec<&str> = v.split(':').collect();
                    if parts.len() != 3 {
                        return Err("--note wants TIME:NOTE:on|off".into());
                    }
                    let at = time_spec(parts[0])?;
                    let on = match parts[2] {
                        "on" => true,
                        "off" => false,
                        other => return Err(format!("bad note state {other}")),
                    };
                    a.notes.push(Note {
                        at,
                        note: num(parts[1])?,
                        on,
                    });
                }
                "--list-params" => a.list_params = true,
                "-h" | "--help" => return Err(USAGE.into()),
                other => return Err(format!("unknown flag {other}\n{USAGE}")),
            }
        }
        if a.block == 0 {
            return Err("--block must be > 0".into());
        }
        Ok(a)
    }
}

/// The host transport of the harness: ticks at the block start plus an offset.
struct Transport {
    sr: f64,
    tempo: f64,
    ppq: f64,
    start_ticks: f64,
    pos: u64,
}

impl Transport {
    fn ticks(&self) -> f64 {
        self.start_ticks + (self.pos as f64 + 0.0) * self.tempo / 60.0 * self.ppq / self.sr
    }
}

/// Runs the command line: builds the input, drives the engine like the harness drives the real
/// plugin, and writes the output.
pub fn run(args: &Args) -> Result<(), String> {
    let mut tr = Transport {
        sr: args.sr as f64,
        tempo: args.tempo,
        ppq: args.ppq as f64,
        start_ticks: args.pos_beats * args.ppq as f64,
        pos: 0,
    };
    let input = match (&args.input, &args.generator) {
        (Some(path), None) => Some(wav::read(path, args.sr)?),
        (None, Some(spec)) => Some(generate(
            spec,
            (args.seconds * args.sr as f64) as usize,
            args.sr as f64,
        )?),
        (None, None) => None,
        _ => return Err("use either --input or --gen".into()),
    };
    if let (Some(path), Some(buf)) = (&args.dump_input, &input) {
        wav::write(path, buf, args.sr)?;
    }
    let state = match &args.state {
        Some(path) => Some(load_state(path)?),
        None => None,
    };

    let mut e = Engine::new();
    e.set_song_ticks(tr.ticks());
    e.set_sample_rate(args.sr as i32);
    e.set_block_size(args.block as i32);
    e.set_time_sig(args.ppq);
    e.set_tempo(args.tempo as f32);
    if let Some(mode) = args.process_mode {
        e.set_process_mode(mode);
    }
    if let Some(blob) = state {
        let used = e.load_state(&blob);
        eprintln!("[render] state consumed {used} of {} bytes", blob.len());
    }
    for &(index, value) in &args.params {
        let r = e.process_param(index as usize, value, REC_UPDATE_VALUE | REC_UPDATE_CONTROL);
        eprintln!("[render] ProcessParam({index}, {value}) -> {r}");
    }
    if args.list_params {
        list_params(&mut e);
    }
    if let Some(path) = &args.save_state {
        std::fs::write(path, e.save_state()).map_err(|err| format!("{path}: {err}"))?;
    }
    if let Some(mut buf) = input {
        e.set_playing(true);
        render(&mut e, &mut tr, &mut buf, args);
        if let Some(path) = &args.output {
            wav::write(path, &buf, args.sr)?;
            eprintln!("[render] wrote {path} ({} frames)", buf.len());
        }
        e.set_playing(false);
    }
    Ok(())
}

fn list_params(e: &mut Engine) {
    println!("index\tname\tvalue\tvalue_text");
    let slot_name = |e: &Engine, bank: usize, slot: usize| -> String {
        let name = &e.banks[bank].slots[slot].name;
        if name.is_empty() {
            format!("Slot {}", slot + 1)
        } else {
            name.clone()
        }
    };
    for (i, p) in PARAMS.iter().enumerate().take(NUM_PARAMS) {
        let value = e.process_param(i, 0, REC_GET_VALUE);
        let text = param_text(i, value, |b, s| slot_name(e, b, s));
        println!("{i}\t{}\t{value}\t{text}", p.fl_name);
    }
    for i in 0..2 {
        let names: Vec<String> = (0..36)
            .map(|v| format!("{v}={}", slot_name(e, i, v)))
            .collect();
        println!("# param {i} slots: {}", names.join(", "));
    }
}

/// A render-time event: a MIDI message or a parameter change.
#[derive(Clone, Copy)]
enum Event {
    Midi(u32),
    Param(i32, i32),
}

/// Renders the buffer in blocks, split at event times, with the events delivered between blocks.
fn render(e: &mut Engine, tr: &mut Transport, buf: &mut [[f32; 2]], args: &Args) {
    let frames = buf.len();
    let sr = args.sr as f64;
    let mut events: Vec<(u64, Event)> = args
        .notes
        .iter()
        .map(|n| {
            let at = sample_at(n.at, sr, args.tempo);
            let status = if n.on { 0x90 } else { 0x80 };
            let vel = if n.on { 100 } else { 0 };
            (at, Event::Midi(status | (n.note as u32) << 8 | vel << 16))
        })
        .collect();
    for &(t, index, value) in &args.param_at {
        events.push((sample_at(t, sr, args.tempo), Event::Param(index, value)));
    }
    // Stable: at equal times, notes come before parameters, each in command line order.
    events.sort_by_key(|e| (e.0, matches!(e.1, Event::Param(..))));
    let mut next_event = 0;
    let mut pos = 0usize;
    while pos < frames {
        while next_event < events.len() && events[next_event].0 as usize <= pos {
            // The original reads the host song position whenever it needs it, so events see the
            // position of their own sample.
            e.set_song_ticks(tr.ticks());
            match events[next_event].1 {
                Event::Midi(msg) => {
                    e.midi_in(msg);
                }
                Event::Param(index, value) => {
                    e.process_param(index as usize, value, REC_UPDATE_VALUE | REC_UPDATE_CONTROL);
                }
            }
            next_event += 1;
        }
        let mut len = args.block.min(frames - pos);
        if let Some(&(at, _)) = events.get(next_event) {
            len = len.min(at as usize - pos);
        }
        e.set_song_ticks(tr.ticks());
        e.render(&mut buf[pos..pos + len]);
        pos += len;
        tr.pos = pos as u64;
    }
}

/// Reads a raw state blob, or the plugin state of an FL `.fst` preset.
pub fn load_state(path: &str) -> Result<Vec<u8>, String> {
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if !data.starts_with(b"FLhd") {
        return Ok(data);
    }
    backspin_engine::fst_state(&data)
        .map(<[u8]>::to_vec)
        .map_err(|e| format!("{path}: {e}"))
}

/// The harness test signals: `impulse:PERIOD`, `sine:HZ`, `ramp[:PERIOD]` (0..1 sawtooth, the
/// whole signal by default, so each value encodes its source position).
pub fn generate(spec: &str, frames: usize, sr: f64) -> Result<Vec<[f32; 2]>, String> {
    let (kind, arg) = spec.split_once(':').unwrap_or((spec, ""));
    let mono: Vec<f32> = match kind {
        "impulse" => {
            let period: usize = num(arg)?;
            (0..frames)
                .map(|i| if i % period.max(1) == 0 { 1.0 } else { 0.0 })
                .collect()
        }
        "sine" => {
            let hz: f64 = num(arg)?;
            (0..frames)
                .map(|i| (0.5 * (std::f64::consts::TAU * hz * i as f64 / sr).sin()) as f32)
                .collect()
        }
        "ramp" => {
            let period: usize = if arg.is_empty() { frames } else { num(arg)? };
            (0..frames)
                .map(|i| (i % period.max(1)) as f32 / period.max(1) as f32)
                .collect()
        }
        _ => return Err(format!("unknown generator {kind}")),
    };
    Ok(mono.iter().map(|&v| [v, v]).collect())
}

fn time_spec(s: &str) -> Result<TimeSpec, String> {
    Ok(if let Some(v) = s.strip_suffix('s') {
        TimeSpec::Seconds(num(v)?)
    } else if let Some(v) = s.strip_suffix('b') {
        TimeSpec::Beats(num(v)?)
    } else {
        TimeSpec::Samples(num(s)?)
    })
}

fn sample_at(t: TimeSpec, sr: f64, tempo: f64) -> u64 {
    match t {
        TimeSpec::Samples(s) => s,
        TimeSpec::Seconds(s) => (s * sr).round() as u64,
        TimeSpec::Beats(b) => (b * 60.0 / tempo * sr).round() as u64,
    }
}

fn num<T: std::str::FromStr>(s: &str) -> Result<T, String> {
    s.trim().parse().map_err(|_| format!("bad number {s:?}"))
}
