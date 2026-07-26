//! Audio transcoding: FLAC ↔ Opus ↔ M4A(AAC)
//!
//! Decode: Symphonia 0.6 (pure Rust, handles all input formats)
//! Encode:
//!   FLAC → flac-bound (libFLAC via vcpkg/apt/brew)
//!   Opus → audiopus   (libopus via vcpkg/apt/brew)
//!   M4A  → fdk-aac    (libfdk-aac via vcpkg/apt/brew)

use crate::cli::{AudioArgs, AudioFmt};
use anyhow::{bail, Context, Result};
use std::path::Path;

// ─── decoded PCM ─────────────────────────────────────────────────────────────

struct Pcm {
    sample_rate: u32,
    channels:    u32,
    /// Interleaved f32 samples, range [-1, 1]
    samples:     Vec<f32>,
}

impl Pcm {
    fn frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    /// Convert to interleaved i16
    fn as_i16(&self) -> Vec<i16> {
        self.samples.iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect()
    }

    /// Convert to interleaved i32 (for FLAC, which uses 16-bit depth stored in i32)
    fn as_i32_16bit(&self) -> Vec<i32> {
        self.samples.iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i32)
            .collect()
    }

    /// Resample to target_rate using linear interpolation.
    /// Good enough for 44.1 kHz → 48 kHz (Opus requirement).
    fn resample_to(&self, target_rate: u32) -> Pcm {
        if self.sample_rate == target_rate {
            return Pcm {
                sample_rate: self.sample_rate,
                channels:    self.channels,
                samples:     self.samples.clone(),
            };
        }
        let ch = self.channels as usize;
        let src_frames = self.frames();
        let ratio = target_rate as f64 / self.sample_rate as f64;
        let dst_frames = (src_frames as f64 * ratio).ceil() as usize;
        let mut out = Vec::with_capacity(dst_frames * ch);

        for dst_f in 0..dst_frames {
            let src_pos = dst_f as f64 / ratio;
            let src_f0 = src_pos.floor() as usize;
            let src_f1 = (src_f0 + 1).min(src_frames - 1);
            let t = src_pos - src_f0 as f64;
            for c in 0..ch {
                let s0 = self.samples[src_f0 * ch + c];
                let s1 = self.samples[src_f1 * ch + c];
                out.push(s0 + (s1 - s0) * t as f32);
            }
        }
        Pcm { sample_rate: target_rate, channels: self.channels, samples: out }
    }
}

// ─── decode ──────────────────────────────────────────────────────────────────

pub fn decode(path: &Path) -> Result<Pcm> {
    use symphonia::core::{
        audio::sample::Sample,
        codecs::audio::AudioDecoderOptions,
        formats::{FormatOptions, probe::Hint},
        io::MediaSourceStream,
        meta::MetadataOptions,
    };
    use symphonia_core::formats::TrackType;

    let file = std::fs::File::open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let mss  = MediaSourceStream::new(Box::new(file), Default::default());
    let hint = Hint::new();

    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .context("Symphonia probe")?;

    let track = format
        .default_track(TrackType::Audio)
        .context("no audio track")?;

    let track_id = track.id;
    let codec_params = track.codec_params
        .as_ref()
        .context("no codec params")?
        .audio()
        .context("not audio codec params")?
        .clone();

    let sample_rate = codec_params.sample_rate.context("unknown sample rate")?;
    let channels    = codec_params.channels.map(|c| c.count() as u32).unwrap_or(2);

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&codec_params, &AudioDecoderOptions::default())
        .context("making decoder")?;

    let mut samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None)    => break,
            Err(e)      => { log::warn!("packet read: {e}"); break; }
        };
        if packet.track_id != track_id { continue; }

        match decoder.decode(&packet) {
            Ok(buf) => {
                let count = buf.samples_interleaved();
                let start = samples.len();
                samples.resize(start + count, f32::MID);
                buf.copy_to_slice_interleaved(&mut samples[start..]);
            }
            Err(symphonia::core::errors::Error::DecodeError(e)) => {
                log::warn!("decode error (continuing): {e}");
            }
            Err(e) => { log::warn!("fatal decode: {e}"); break; }
        }
    }

    log::info!(
        "decoded {} frames  {} Hz  {} ch",
        samples.len() / channels as usize, sample_rate, channels
    );
    Ok(Pcm { sample_rate, channels, samples })
}

// ─── FLAC encode ─────────────────────────────────────────────────────────────

fn encode_flac(pcm: &Pcm, path: &Path, compression: u8) -> Result<()> {
    use flac_bound::{FlacEncoder, WriteWrapper};

    let out_file = std::fs::File::create(path)
        .with_context(|| format!("creating {}", path.display()))?;
    let mut buf_writer = std::io::BufWriter::new(out_file);
    let mut out_wrap = WriteWrapper(&mut buf_writer);

    let mut enc = FlacEncoder::new()
        .context("FlacEncoder::new failed — is libFLAC installed?")?
        .channels(pcm.channels)
        .bits_per_sample(16)
        .sample_rate(pcm.sample_rate)
        .compression_level(compression as u32)
        .init_write(&mut out_wrap)
        .map_err(|e| anyhow::anyhow!("FLAC init_write: {:?}", e))?;

    let i32_samples = pcm.as_i32_16bit();
    enc.process_interleaved(&i32_samples, pcm.frames() as u32)
        .map_err(|_| anyhow::anyhow!("FLAC process_interleaved failed"))?;

    enc.finish()
        .map_err(|e| anyhow::anyhow!("FLAC finish failed: {:?}", e.state()))?;

    Ok(())
}

// ─── Opus encode ─────────────────────────────────────────────────────────────

fn encode_opus(pcm: &Pcm, path: &Path, bitrate_kbps: u32) -> Result<()> {
    use audiopus::{coder::Encoder, Application, Channels, SampleRate, Bitrate};

    // Opus requires 48 kHz
    let pcm48 = pcm.resample_to(48_000);

    let channels = match pcm48.channels {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        n => bail!("Opus supports 1 or 2 channels; got {n}. Mix down first."),
    };

    let mut enc = Encoder::new(SampleRate::Hz48000, channels, Application::Audio)
        .context("Opus encoder init — is libopus installed?")?;

    enc.set_bitrate(Bitrate::BitsPerSecond((bitrate_kbps * 1000) as i32))
        .context("setting Opus bitrate")?;

    // 20 ms frame at 48 kHz = 960 samples per channel
    const FRAME_SIZE: usize = 960;
    let ch = pcm48.channels as usize;
    let i16_samples = pcm48.as_i16();

    // Simple Ogg-less raw Opus stream: each frame preceded by a u32 length.
    // For proper .opus files an Ogg container is needed; for interop we
    // write a simple length-prefixed binary that can be re-muxed later.
    // TODO: wrap in Ogg using the `ogg` crate for spec-compliant .opus files.
    let mut out_bytes: Vec<u8> = Vec::new();
    let mut packet_buf = vec![0u8; 4000];

    for chunk in i16_samples.chunks(FRAME_SIZE * ch) {
        let mut frame = chunk.to_vec();
        frame.resize(FRAME_SIZE * ch, 0); // pad last frame

        let n = enc.encode(&frame, &mut packet_buf)
            .context("Opus encode frame")?;

        // Write length-prefixed packet
        out_bytes.extend_from_slice(&(n as u32).to_le_bytes());
        out_bytes.extend_from_slice(&packet_buf[..n]);
    }

    std::fs::write(path, &out_bytes)
        .with_context(|| format!("writing {}", path.display()))
}

// ─── M4A / AAC encode ────────────────────────────────────────────────────────

fn encode_m4a(pcm: &Pcm, path: &Path, bitrate_kbps: u32) -> Result<()> {
    use fdk_aac::enc::{Encoder, EncoderParams, BitRate, ChannelMode, AudioObjectType, Transport};

    if pcm.channels > 2 {
        bail!("fdk-aac supports up to 2 channels; got {}", pcm.channels);
    }

    let channel_mode = if pcm.channels == 1 {
        ChannelMode::Mono
    } else {
        ChannelMode::Stereo
    };

    let params = EncoderParams {
        bit_rate:          BitRate::Cbr(bitrate_kbps * 1000),
        sample_rate:       pcm.sample_rate,
        transport:         Transport::Adts,   // ADTS frames, easy to write raw
        channels:          channel_mode,
        audio_object_type: AudioObjectType::Mpeg4LowComplexity,
    };

    let enc = Encoder::new(params)
        .map_err(|e| anyhow::anyhow!("fdk-aac encoder init: {:?} — is libfdk-aac installed?", e))?;

    // fdk-aac frame size for LC at most sample rates is 1024 samples/channel
    const FRAME_SIZE: usize = 1024;
    let ch = pcm.channels as usize;
    let i16_samples = pcm.as_i16();

    let mut adts_frames: Vec<u8> = Vec::new();
    let mut out_buf = vec![0u8; 8192];

    for chunk in i16_samples.chunks(FRAME_SIZE * ch) {
        let mut frame = chunk.to_vec();
        frame.resize(FRAME_SIZE * ch, 0);

        let info = enc.encode(&frame, &mut out_buf)
            .map_err(|e| anyhow::anyhow!("fdk-aac encode: {:?}", e))?;

        adts_frames.extend_from_slice(&out_buf[..info.output_size]);
    }

    // Write raw ADTS stream. Rename to .aac if you want bare AAC.
    // A proper M4A would wrap this in an MPEG-4 container (isom/mp4).
    // For now we write ADTS which players accept when named .m4a or .aac.
    std::fs::write(path, &adts_frames)
        .with_context(|| format!("writing {}", path.display()))
}

// ─── public entry point ──────────────────────────────────────────────────────

pub fn run(args: AudioArgs) -> Result<()> {
    let fmt = args.target_fmt()?;

    log::info!("decode  {}", args.input.display());
    let pcm = decode(&args.input)?;

    log::info!(
        "encode  {:?}  {}  ({} kbps / compression {})",
        fmt, args.output.display(), args.bitrate, args.compression
    );

    match fmt {
        AudioFmt::Flac => encode_flac(&pcm, &args.output, args.compression),
        AudioFmt::Opus => encode_opus(&pcm, &args.output, args.bitrate),
        AudioFmt::M4a  => encode_m4a(&pcm, &args.output, args.bitrate),
    }?;

    println!(
        "{} → {}  ({} Hz  {} ch  {} frames)",
        args.input.display(),
        args.output.display(),
        pcm.sample_rate,
        pcm.channels,
        pcm.frames(),
    );
    Ok(())
}
