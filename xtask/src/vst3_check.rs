//! A minimal VST3 host for `cargo xtask plugin-check`: raw vtable calls for the few interfaces it
//! needs (IPluginFactory, IComponent, IAudioProcessor) and a host-side IBStream.

use crate::plugin_check::{Check, Host, state_json};
use std::ffi::{CString, c_char, c_void};

const BINARY: &str = "target/bundled/Backspin.vst3/Contents/MacOS/Backspin";
const CLASS_ID: [u8; 16] = *b"BackspinFxPlugin";
const K_RESULT_OK: i32 = 0;

/// Interface IDs in the byte order of non-Windows platforms (the four 32-bit words big endian).
const IID_ICOMPONENT: [u8; 16] = uid(0xE831FF31, 0xF2D54301, 0x928EBBEE, 0x25697802);
const IID_IAUDIOPROCESSOR: [u8; 16] = uid(0x42043F99, 0xB7DA453C, 0xA569E79D, 0x9AAEC33D);
const IID_IBSTREAM: [u8; 16] = uid(0xC3BF6EA2, 0x30994752, 0x9B6BF990, 0x1EE33E9B);
const IID_FUNKNOWN: [u8; 16] = uid(0x00000000, 0x00000000, 0xC0000000, 0x00000046);

/// ProcessContext state flags.
const K_PLAYING: u32 = 1 << 1;
const K_PROJECT_TIME_MUSIC_VALID: u32 = 1 << 9;
const K_TEMPO_VALID: u32 = 1 << 10;

const fn uid(a: u32, b: u32, c: u32, d: u32) -> [u8; 16] {
    let (a, b, c, d) = (
        a.to_be_bytes(),
        b.to_be_bytes(),
        c.to_be_bytes(),
        d.to_be_bytes(),
    );
    [
        a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3], c[0], c[1], c[2], c[3], d[0], d[1], d[2],
        d[3],
    ]
}

type QueryInterface = unsafe extern "C" fn(*mut c_void, *const [u8; 16], *mut *mut c_void) -> i32;
type AddRef = unsafe extern "C" fn(*mut c_void) -> u32;

#[repr(C)]
struct FactoryVtbl {
    query_interface: QueryInterface,
    add_ref: AddRef,
    release: AddRef,
    get_factory_info: usize,
    count_classes: usize,
    get_class_info: usize,
    create_instance: unsafe extern "C" fn(
        *mut c_void,
        *const [u8; 16],
        *const [u8; 16],
        *mut *mut c_void,
    ) -> i32,
}

#[repr(C)]
struct ComponentVtbl {
    query_interface: QueryInterface,
    add_ref: AddRef,
    release: AddRef,
    initialize: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32,
    terminate: unsafe extern "C" fn(*mut c_void) -> i32,
    get_controller_class_id: usize,
    set_io_mode: usize,
    get_bus_count: usize,
    get_bus_info: usize,
    get_routing_info: usize,
    activate_bus: usize,
    set_active: unsafe extern "C" fn(*mut c_void, u8) -> i32,
    set_state: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32,
    get_state: usize,
}

#[repr(C)]
struct ProcessorVtbl {
    query_interface: QueryInterface,
    add_ref: AddRef,
    release: AddRef,
    set_bus_arrangements: usize,
    get_bus_arrangement: usize,
    can_process_sample_size: usize,
    get_latency_samples: unsafe extern "C" fn(*mut c_void) -> u32,
    setup_processing: unsafe extern "C" fn(*mut c_void, *const ProcessSetup) -> i32,
    set_processing: unsafe extern "C" fn(*mut c_void, u8) -> i32,
    process: unsafe extern "C" fn(*mut c_void, *mut ProcessData) -> i32,
    get_tail_samples: usize,
}

#[repr(C)]
struct ProcessSetup {
    process_mode: i32,
    symbolic_sample_size: i32,
    max_samples_per_block: i32,
    sample_rate: f64,
}

#[repr(C)]
struct AudioBusBuffers {
    num_channels: i32,
    silence_flags: u64,
    buffers: *mut *mut f32,
}

#[repr(C)]
struct ProcessData {
    process_mode: i32,
    symbolic_sample_size: i32,
    num_samples: i32,
    num_inputs: i32,
    num_outputs: i32,
    inputs: *mut AudioBusBuffers,
    outputs: *mut AudioBusBuffers,
    input_param_changes: *mut c_void,
    output_param_changes: *mut c_void,
    input_events: *mut c_void,
    output_events: *mut c_void,
    context: *mut ProcessContext,
}

#[repr(C)]
#[derive(Default)]
struct ProcessContext {
    state: u32,
    sample_rate: f64,
    project_time_samples: i64,
    system_time: i64,
    continuous_time_samples: i64,
    project_time_music: f64,
    bar_position_music: f64,
    cycle_start_music: f64,
    cycle_end_music: f64,
    tempo: f64,
    time_sig_num: i32,
    time_sig_den: i32,
    key_note: u8,
    root_note: u8,
    chord_mask: i16,
    smpte_offset_subframes: i32,
    frames_per_second: u32,
    frame_rate_flags: u32,
    samples_to_next_clock: i64,
}

/// A host-side IBStream over a byte vector.
#[repr(C)]
struct Stream {
    vtbl: *const StreamVtbl,
    data: Vec<u8>,
    pos: i64,
}

#[repr(C)]
struct StreamVtbl {
    query_interface: QueryInterface,
    add_ref: AddRef,
    release: AddRef,
    read: unsafe extern "C" fn(*mut Stream, *mut c_void, i32, *mut i32) -> i32,
    write: unsafe extern "C" fn(*mut Stream, *const c_void, i32, *mut i32) -> i32,
    seek: unsafe extern "C" fn(*mut Stream, i64, i32, *mut i64) -> i32,
    tell: unsafe extern "C" fn(*mut Stream, *mut i64) -> i32,
}

static STREAM_VTBL: StreamVtbl = StreamVtbl {
    query_interface: stream_query_interface,
    add_ref: stream_ref,
    release: stream_ref,
    read: stream_read,
    write: stream_write,
    seek: stream_seek,
    tell: stream_tell,
};

unsafe extern "C" fn stream_query_interface(
    this: *mut c_void,
    iid: *const [u8; 16],
    obj: *mut *mut c_void,
) -> i32 {
    unsafe {
        if *iid == IID_IBSTREAM || *iid == IID_FUNKNOWN {
            *obj = this;
            return K_RESULT_OK;
        }
        *obj = std::ptr::null_mut();
    }
    -1
}

unsafe extern "C" fn stream_ref(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "C" fn stream_read(
    this: *mut Stream,
    buf: *mut c_void,
    n: i32,
    read: *mut i32,
) -> i32 {
    unsafe {
        let s = &mut *this;
        let k = (n as i64).min(s.data.len() as i64 - s.pos).max(0) as usize;
        std::ptr::copy_nonoverlapping(s.data.as_ptr().add(s.pos as usize), buf as *mut u8, k);
        s.pos += k as i64;
        if !read.is_null() {
            *read = k as i32;
        }
    }
    K_RESULT_OK
}

unsafe extern "C" fn stream_write(
    _this: *mut Stream,
    _buf: *const c_void,
    _n: i32,
    _written: *mut i32,
) -> i32 {
    -1
}

unsafe extern "C" fn stream_seek(this: *mut Stream, pos: i64, mode: i32, result: *mut i64) -> i32 {
    unsafe {
        let s = &mut *this;
        s.pos = match mode {
            0 => pos,
            1 => s.pos + pos,
            _ => s.data.len() as i64 + pos,
        };
        if !result.is_null() {
            *result = s.pos;
        }
    }
    K_RESULT_OK
}

unsafe extern "C" fn stream_tell(this: *mut Stream, pos: *mut i64) -> i32 {
    unsafe { *pos = (*this).pos };
    K_RESULT_OK
}

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// The loaded VST3 library.
pub struct Vst3 {
    factory: *mut c_void,
}

impl Vst3 {
    pub unsafe fn open() -> Result<Self, String> {
        unsafe {
            let path = CString::new(BINARY).unwrap();
            let h = dlopen(path.as_ptr(), 2);
            if h.is_null() {
                return Err(format!("dlopen {BINARY} failed"));
            }
            let entry = dlsym(h, c"bundleEntry".as_ptr());
            if entry.is_null() {
                return Err("no bundleEntry".into());
            }
            let entry: unsafe extern "C" fn(*mut c_void) -> bool = std::mem::transmute(entry);
            if !entry(std::ptr::null_mut()) {
                return Err("bundleEntry failed".into());
            }
            let get = dlsym(h, c"GetPluginFactory".as_ptr());
            if get.is_null() {
                return Err("no GetPluginFactory".into());
            }
            let get: unsafe extern "C" fn() -> *mut c_void = std::mem::transmute(get);
            let factory = get();
            if factory.is_null() {
                return Err("GetPluginFactory returned null".into());
            }
            Ok(Self { factory })
        }
    }
}

unsafe fn vtbl<T>(obj: *mut c_void) -> &'static T {
    unsafe { &**(obj as *mut *const T) }
}

impl Host for Vst3 {
    unsafe fn render(
        &self,
        blob: &[u8],
        params: &[i32; 9],
        input: &[[f32; 2]],
        c: &Check,
    ) -> Result<Vec<[f32; 2]>, String> {
        unsafe {
            let f = vtbl::<FactoryVtbl>(self.factory);
            let mut comp = std::ptr::null_mut();
            if (f.create_instance)(self.factory, &CLASS_ID, &IID_ICOMPONENT, &mut comp)
                != K_RESULT_OK
            {
                return Err("createInstance failed".into());
            }
            let cv = vtbl::<ComponentVtbl>(comp);
            (cv.initialize)(comp, std::ptr::null_mut());
            let mut stream = Box::new(Stream {
                vtbl: &STREAM_VTBL,
                data: state_json(blob, params).into_bytes(),
                pos: 0,
            });
            if (cv.set_state)(comp, &mut *stream as *mut Stream as *mut c_void) != K_RESULT_OK {
                return Err("setState failed".into());
            }
            let mut proc_ = std::ptr::null_mut();
            if (cv.query_interface)(comp, &IID_IAUDIOPROCESSOR, &mut proc_) != K_RESULT_OK {
                return Err("no IAudioProcessor".into());
            }
            let pv = vtbl::<ProcessorVtbl>(proc_);
            let setup = ProcessSetup {
                process_mode: 0,
                symbolic_sample_size: 0,
                max_samples_per_block: c.block as i32,
                sample_rate: c.sr as f64,
            };
            if (pv.setup_processing)(proc_, &setup) != K_RESULT_OK {
                return Err("setupProcessing failed".into());
            }
            (cv.set_active)(comp, 1);
            (pv.set_processing)(proc_, 1);
            if (pv.get_latency_samples)(proc_) != 15 {
                return Err("latency is not 15 samples".into());
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
                let mut ain = AudioBusBuffers {
                    num_channels: 2,
                    silence_flags: 0,
                    buffers: in_ptrs.as_mut_ptr(),
                };
                let mut aout = AudioBusBuffers {
                    num_channels: 2,
                    silence_flags: 0,
                    buffers: out_ptrs.as_mut_ptr(),
                };
                let mut ctx = ProcessContext {
                    state: K_PLAYING | K_PROJECT_TIME_MUSIC_VALID | K_TEMPO_VALID,
                    sample_rate: c.sr as f64,
                    project_time_samples: pos as i64,
                    project_time_music: pos as f64 * c.tempo / 60.0 / c.sr as f64,
                    tempo: c.tempo,
                    time_sig_num: 4,
                    time_sig_den: 4,
                    ..Default::default()
                };
                let mut data = ProcessData {
                    process_mode: 0,
                    symbolic_sample_size: 0,
                    num_samples: n as i32,
                    num_inputs: 1,
                    num_outputs: 1,
                    inputs: &mut ain,
                    outputs: &mut aout,
                    input_param_changes: std::ptr::null_mut(),
                    output_param_changes: std::ptr::null_mut(),
                    input_events: std::ptr::null_mut(),
                    output_events: std::ptr::null_mut(),
                    context: &mut ctx,
                };
                if (pv.process)(proc_, &mut data) != K_RESULT_OK {
                    return Err("process failed".into());
                }
                for i in 0..n {
                    out[pos + i] = [outl[i], outr[i]];
                }
                pos += n;
            }
            (pv.set_processing)(proc_, 0);
            (cv.set_active)(comp, 0);
            (cv.terminate)(comp);
            (pv.release)(proc_);
            (cv.release)(comp);
            drop(stream);
            Ok(out)
        }
    }
}
