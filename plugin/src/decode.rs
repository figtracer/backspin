//! Audio file decoding for "Analyze audio file". FL decodes with its own loader; this uses
//! Symphonia, so the frames of compressed formats can differ (a known difference).

use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Decodes a file into stereo frames (channel 0 and 1), the channel count and the sample rate.
pub fn decode(path: &Path) -> Result<(Vec<[f32; 2]>, usize, f64), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| e.to_string())?;
    let mut format = probed.format;
    let track = format.default_track().ok_or("no audio track")?;
    let id = track.id;
    let params = track.codec_params.clone();
    let channels = params.channels.map_or(1, |c| c.count());
    let sr = params.sample_rate.ok_or("unknown sample rate")? as f64;
    let mut dec = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(|e| e.to_string())?;
    let mut frames = Vec::new();
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != id {
            continue;
        }
        let Ok(buf) = dec.decode(&packet) else { continue };
        let mut s = SampleBuffer::<f32>::new(buf.capacity() as u64, *buf.spec());
        s.copy_interleaved_ref(buf);
        for f in s.samples().chunks(channels.max(1)) {
            frames.push([f[0], if channels > 1 { f[1] } else { f[0] }]);
        }
    }
    Ok((frames, channels, sr))
}
