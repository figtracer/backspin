//! `cargo xtask plugin-check`: loads the bundled CLAP plugin in a minimal host, loads factory
//! presets through the plugin's state, renders, and compares with `backspin-render`.
//!
//! The host gives the transport like a DAW that plays from the song start: tempo, beat position
//! and the playing flag on every block. Notes go in as CLAP note events at block starts, where the
//! harness block grid and the host grid agree.

use backspin_engine::{Engine, fst_state};
use backspin_render::{generate, wav};
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::*;
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
use clap_sys::factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory};
use clap_sys::fixedpoint::CLAP_BEATTIME_FACTOR;
use clap_sys::host::clap_host;
use clap_sys::process::clap_process;
use clap_sys::stream::clap_istream;
use clap_sys::version::CLAP_VERSION;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::Path;
use std::process::Command;

const BUNDLE: &str = "target/bundled/Backspin.clap";
const BINARY: &str = "target/bundled/Backspin.clap/Contents/MacOS/Backspin";
const PRESET_DIR: &str = "/Applications/FL Studio 2026.app/Contents/Resources/FL/Data/Patches/Plugin presets/Effects/Gross Beat";
const PLUGIN_ID: &CStr = c"com.figtracer.backspin";

/// One check: a preset, slot parameters, a render setup and optional notes `(frame, note, on)`.
pub(crate) struct Check {
    pub preset: &'static str,
    pub time: i32,
    pub volume: i32,
    pub input: &'static str,
    pub sr: u32,
    pub tempo: f64,
    pub block: usize,
    pub notes: Vec<(usize, u8, bool)>,
}

pub fn run(_args: &[String]) -> Result<(), String> {
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "backspin-render"])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("building backspin-render failed".into());
    }
    if !Path::new(BINARY).exists() {
        return Err(format!(
            "{BUNDLE} is missing; run cargo xtask bundle backspin --release"
        ));
    }
    let dir = Path::new("target/plugin-check");
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let lib = unsafe { Library::open(BINARY)? };
    let mut checks = Vec::new();
    let presets = [
        ("Default", 0, 0),
        ("Flanging", 3, 0),
        ("Juggling Science", 5, 2),
        ("Momentary", 4, 7),
        ("Patterns", 0, 12),
        ("Pitch shifter", 6, 3),
        ("Repeater", 20, 9),
        ("Stutter", 8, 25),
        ("Turntablist", 11, 4),
    ];
    for (preset, time, volume) in presets {
        for input in ["ramp", "noise"] {
            checks.push(Check {
                preset,
                time,
                volume,
                input,
                sr: 48000,
                tempo: 120.0,
                block: 512,
                notes: vec![],
            });
        }
        checks.push(Check {
            preset,
            time,
            volume,
            input: "ramp",
            sr: 44100,
            tempo: 174.0,
            block: 333,
            notes: vec![],
        });
    }
    checks.push(Check {
        preset: "Momentary",
        time: 0,
        volume: 0,
        input: "ramp",
        sr: 48000,
        tempo: 120.0,
        block: 512,
        notes: vec![
            (512 * 50, 61, true),
            (512 * 140, 61, false),
            (512 * 200, 30, true),
            (512 * 260, 30, false),
        ],
    });
    let vst3 = unsafe { crate::vst3_check::Vst3::open()? };
    let hosts: [(&str, &dyn Host); 2] = [("clap", &lib), ("vst3", &vst3)];
    let mut failed = 0;
    let mut total = 0;
    for (format, host) in hosts {
        for c in &checks {
            if format == "vst3" && !c.notes.is_empty() {
                continue;
            }
            total += 1;
            let r = unsafe { check(host, dir, c) };
            let tag = format!(
                "{format} {} t={} v={} in={} sr={} bpm={} blk={} notes={}",
                c.preset,
                c.time,
                c.volume,
                c.input,
                c.sr,
                c.tempo,
                c.block,
                c.notes.len()
            );
            match r {
                Ok(None) => println!("pass {tag}"),
                Ok(Some((max, first))) => {
                    failed += 1;
                    println!("FAIL {tag}: max {max:.3e} first sample {first}");
                }
                Err(e) => {
                    failed += 1;
                    println!("ERROR {tag}: {e}");
                }
            }
        }
    }
    if failed > 0 {
        return Err(format!("{failed} of {total} checks failed"));
    }
    println!("all {total} checks pass");
    Ok(())
}

/// Renders one check through the plugin and through `backspin-render`, and returns the max
/// difference and the first differing sample, or `None` if they are bit-identical.
unsafe fn check(lib: &dyn Host, dir: &Path, c: &Check) -> Result<Option<(f32, usize)>, String> {
    let seconds = 4.0;
    let frames = (seconds * c.sr as f64) as usize;
    let input = if c.input == "noise" {
        let mut x = 0x5eed_1234u64;
        (0..frames)
            .map(|_| {
                let mut r = || {
                    x = x
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((x >> 33) as f64 / (1u64 << 31) as f64 * 2.0 - 1.0) as f32 * 0.5
                };
                [r(), r()]
            })
            .collect()
    } else {
        generate(c.input, frames, c.sr as f64)?
    };
    let input_path = dir.join("input.wav").display().to_string();
    wav::write(&input_path, &input, c.sr)?;
    let preset = Path::new(PRESET_DIR).join(format!("{}.fst", c.preset));

    // The reference: backspin-render with the same preset, parameters and notes.
    let out_path = dir.join("render.wav").display().to_string();
    let mut args = vec![
        "--input".to_string(),
        input_path,
        "--output".into(),
        out_path.clone(),
        "--sr".into(),
        c.sr.to_string(),
        "--tempo".into(),
        c.tempo.to_string(),
        "--block".into(),
        c.block.to_string(),
        "--state".into(),
        preset.display().to_string(),
        "--param".into(),
        format!("0={}", c.time),
        "--param".into(),
        format!("1={}", c.volume),
    ];
    for &(at, note, on) in &c.notes {
        args.extend([
            "--note".into(),
            format!("{at}:{note}:{}", if on { "on" } else { "off" }),
        ]);
    }
    let out = Command::new("target/release/backspin-render")
        .args(&args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let want = wav::read(&out_path, c.sr)?;

    // The plugin: the preset goes in as nih-plug state, with the host parameters set to the preset
    // values and the two slot parameters.
    let data = std::fs::read(&preset).map_err(|e| e.to_string())?;
    let blob = fst_state(&data).map_err(|e| e.to_string())?.to_vec();
    let mut e = Engine::new();
    e.load_state(&blob);
    let mut params = e.params;
    params[0] = c.time;
    params[1] = c.volume;
    let got = unsafe { lib.render(&blob, &params, &input, c)? };

    let mut first = None;
    let mut max = 0f32;
    for (k, (a, b)) in got.iter().zip(&want).enumerate() {
        for ch in 0..2 {
            if a[ch].to_bits() != b[ch].to_bits() {
                max = max.max((a[ch] - b[ch]).abs());
                first.get_or_insert(k);
            }
        }
    }
    if got.len() != want.len() {
        return Err(format!("length {} != {}", got.len(), want.len()));
    }
    Ok(first.map(|f| (max, f)))
}

/// The nih-plug state JSON for a blob and parameter values.
pub(crate) fn state_json(blob: &[u8], params: &[i32; 9]) -> String {
    let ids = [
        "time_slot",
        "volume_slot",
        "time_mix",
        "volume_mix",
        "attack",
        "release",
        "tension",
        "hold",
        "scratch",
    ];
    let p: Vec<String> = ids
        .iter()
        .zip(params)
        .map(|(id, v)| format!("\"{id}\":{{\"i32\":{v}}}"))
        .collect();
    let hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
    let field = format!("\"\\\"{hex}\\\"\"");
    format!(
        "{{\"version\":\"check\",\"params\":{{{}}},\"fields\":{{\"gross-beat-state\":{field}}}}}",
        p.join(",")
    )
}

/// A plugin format host that renders one check.
pub(crate) trait Host {
    /// Creates an instance, loads the state, plays the input from the song start and returns
    /// the output.
    unsafe fn render(
        &self,
        blob: &[u8],
        params: &[i32; 9],
        input: &[[f32; 2]],
        c: &Check,
    ) -> Result<Vec<[f32; 2]>, String>;
}

/// The loaded CLAP library.
struct Library {
    entry: *const clap_plugin_entry,
}

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

impl Library {
    unsafe fn open(path: &str) -> Result<Self, String> {
        let cpath = CString::new(path).unwrap();
        let h = unsafe { dlopen(cpath.as_ptr(), 2) };
        if h.is_null() {
            return Err(format!(
                "dlopen: {}",
                unsafe { CStr::from_ptr(dlerror()) }.to_string_lossy()
            ));
        }
        let entry = unsafe { dlsym(h, c"clap_entry".as_ptr()) } as *const clap_plugin_entry;
        if entry.is_null() {
            return Err("no clap_entry".into());
        }
        let bundle =
            CString::new(std::fs::canonicalize(BUNDLE).unwrap().display().to_string()).unwrap();
        if !unsafe { ((*entry).init.unwrap())(bundle.as_ptr()) } {
            return Err("clap_entry.init failed".into());
        }
        Ok(Self { entry })
    }
}

impl Host for Library {
    unsafe fn render(
        &self,
        blob: &[u8],
        params: &[i32; 9],
        input: &[[f32; 2]],
        c: &Check,
    ) -> Result<Vec<[f32; 2]>, String> {
        unsafe {
            let factory = ((*self.entry).get_factory.unwrap())(CLAP_PLUGIN_FACTORY_ID.as_ptr())
                as *const clap_plugin_factory;
            let host = Box::new(clap_host {
                clap_version: CLAP_VERSION,
                host_data: std::ptr::null_mut(),
                name: c"backspin plugin-check".as_ptr(),
                vendor: c"".as_ptr(),
                url: c"".as_ptr(),
                version: c"1".as_ptr(),
                get_extension: Some(host_get_extension),
                request_restart: Some(host_noop),
                request_process: Some(host_noop),
                request_callback: Some(host_noop),
            });
            let plugin = ((*factory).create_plugin.unwrap())(factory, &*host, PLUGIN_ID.as_ptr());
            if plugin.is_null() {
                return Err("create_plugin failed".into());
            }
            let p = &*plugin;
            if !(p.init.unwrap())(plugin) {
                return Err("init failed".into());
            }
            let state = (p.get_extension.unwrap())(plugin, CLAP_EXT_STATE.as_ptr())
                as *const clap_plugin_state;
            // nih-plug prefixes its CLAP state with the JSON length as a u64.
            let json = state_json(blob, params).into_bytes();
            let json = [&(json.len() as u64).to_le_bytes()[..], &json].concat();
            let mut reader = Reader { data: json, pos: 0 };
            let stream = clap_istream {
                ctx: &mut reader as *mut Reader as *mut c_void,
                read: Some(stream_read),
            };
            if !((*state).load.unwrap())(plugin, &stream) {
                return Err("state load failed".into());
            }
            if !(p.activate.unwrap())(plugin, c.sr as f64, 1, c.block as u32) {
                return Err("activate failed".into());
            }
            if !(p.start_processing.unwrap())(plugin) {
                return Err("start_processing failed".into());
            }
            let mut out = vec![[0f32; 2]; input.len()];
            let (mut inl, mut inr) = (vec![0f32; c.block], vec![0f32; c.block]);
            let (mut outl, mut outr) = (vec![0f32; c.block], vec![0f32; c.block]);
            let mut pos = 0;
            while pos < input.len() {
                let n = c.block.min(input.len() - pos);
                for i in 0..n {
                    inl[i] = input[pos + i][0];
                    inr[i] = input[pos + i][1];
                }
                let mut in_ptrs = [inl.as_mut_ptr(), inr.as_mut_ptr()];
                let mut out_ptrs = [outl.as_mut_ptr(), outr.as_mut_ptr()];
                let ain = clap_audio_buffer {
                    data32: in_ptrs.as_mut_ptr(),
                    data64: std::ptr::null_mut(),
                    channel_count: 2,
                    latency: 0,
                    constant_mask: 0,
                };
                let mut aout = clap_audio_buffer {
                    data32: out_ptrs.as_mut_ptr(),
                    data64: std::ptr::null_mut(),
                    channel_count: 2,
                    latency: 0,
                    constant_mask: 0,
                };
                let beats = pos as f64 * c.tempo / 60.0 / c.sr as f64;
                let transport = clap_event_transport {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_transport>() as u32,
                        time: 0,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: CLAP_EVENT_TRANSPORT,
                        flags: 0,
                    },
                    flags: CLAP_TRANSPORT_HAS_TEMPO
                        | CLAP_TRANSPORT_HAS_BEATS_TIMELINE
                        | CLAP_TRANSPORT_IS_PLAYING,
                    song_pos_beats: (beats * CLAP_BEATTIME_FACTOR as f64).round() as i64,
                    song_pos_seconds: 0,
                    tempo: c.tempo,
                    tempo_inc: 0.0,
                    loop_start_beats: 0,
                    loop_end_beats: 0,
                    loop_start_seconds: 0,
                    loop_end_seconds: 0,
                    bar_start: 0,
                    bar_number: 0,
                    tsig_num: 4,
                    tsig_denom: 4,
                };
                let notes: Vec<clap_event_note> = c
                    .notes
                    .iter()
                    .filter(|(at, _, _)| *at >= pos && *at < pos + n)
                    .map(|&(at, key, on)| clap_event_note {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_note>() as u32,
                            time: (at - pos) as u32,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: if on {
                                CLAP_EVENT_NOTE_ON
                            } else {
                                CLAP_EVENT_NOTE_OFF
                            },
                            flags: 0,
                        },
                        note_id: -1,
                        port_index: 0,
                        channel: 0,
                        key: key as i16,
                        velocity: if on { 100.0 / 127.0 } else { 0.0 },
                    })
                    .collect();
                let events = Events(notes);
                let in_events = clap_input_events {
                    ctx: &events as *const Events as *mut c_void,
                    size: Some(events_size),
                    get: Some(events_get),
                };
                let out_events = clap_output_events {
                    ctx: std::ptr::null_mut(),
                    try_push: Some(events_push),
                };
                let process = clap_process {
                    steady_time: pos as i64,
                    frames_count: n as u32,
                    transport: &transport,
                    audio_inputs: &ain,
                    audio_outputs: &mut aout,
                    audio_inputs_count: 1,
                    audio_outputs_count: 1,
                    in_events: &in_events,
                    out_events: &out_events,
                };
                (p.process.unwrap())(plugin, &process);
                for i in 0..n {
                    out[pos + i] = [outl[i], outr[i]];
                }
                pos += n;
            }
            (p.stop_processing.unwrap())(plugin);
            (p.deactivate.unwrap())(plugin);
            (p.destroy.unwrap())(plugin);
            drop(host);
            Ok(out)
        }
    }
}

struct Reader {
    data: Vec<u8>,
    pos: usize,
}

struct Events(Vec<clap_event_note>);

unsafe extern "C" fn stream_read(
    stream: *const clap_istream,
    buffer: *mut c_void,
    size: u64,
) -> i64 {
    unsafe {
        let r = &mut *((*stream).ctx as *mut Reader);
        let n = (size as usize).min(r.data.len() - r.pos);
        std::ptr::copy_nonoverlapping(r.data.as_ptr().add(r.pos), buffer as *mut u8, n);
        r.pos += n;
        n as i64
    }
}

unsafe extern "C" fn events_size(list: *const clap_input_events) -> u32 {
    unsafe { (*((*list).ctx as *const Events)).0.len() as u32 }
}

unsafe extern "C" fn events_get(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    unsafe {
        let events = &*((*list).ctx as *const Events);
        &events.0[index as usize].header
    }
}

unsafe extern "C" fn events_push(
    _list: *const clap_output_events,
    _event: *const clap_event_header,
) -> bool {
    true
}

unsafe extern "C" fn host_get_extension(
    _host: *const clap_host,
    _id: *const c_char,
) -> *const c_void {
    std::ptr::null()
}

unsafe extern "C" fn host_noop(_host: *const clap_host) {}
