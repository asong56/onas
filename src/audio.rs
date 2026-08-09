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

fn decode(path: &Path) -> Result<Pcm> {
    use symphonia::core::{
        audio::sample::Sample,
        codecs::audio::AudioDecoderOptions,
        formats::{FormatOptions, probe::Hint},
        io::MediaSourceStream,
        meta::MetadataOptions,
    };
    use symphonia::core::formats::TrackType;

    let file = std::fs::File::open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let mss  = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    // Symphonia's probe falls back to content sniffing without this, but the
    // extension hint lets it pick the right demuxer directly — including
    // container formats like MKV/WebM, so an audio track can be pulled
    // straight out of a video file (codec support inside still applies).
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

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

    // The bundled symphonia has never shipped a native Opus decoder — but
    // libopus is already statically linked in for Opus *encoding* below
    // (`audiopus`), so decode reuses that same already-linked library
    // instead of pulling in a second Opus implementation. Detected via the
    // codec id's Debug text rather than a `well_known::CODEC_ID_OPUS`
    // import, since the exact constant path has moved between symphonia
    // point releases and this is resilient to that.
    let is_opus = format!("{:?}", codec_params.codec).to_ascii_lowercase().contains("opus");

    let mut samples: Vec<f32> = Vec::new();
    let (sample_rate, channels) = if is_opus {
        decode_opus_via_audiopus(&mut *format, track_id, &codec_params, &mut samples)?
    } else {
        let sample_rate = codec_params.sample_rate.context("unknown sample rate")?;
        let channels    = codec_params.channels.as_ref().map(|c| c.count() as u32).unwrap_or(2);
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&codec_params, &AudioDecoderOptions::default())
            .with_context(|| format!("no decoder for {:?}", codec_params.codec))?;

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
        (sample_rate, channels)
    };

    log::info!(
        "decoded {} frames  {} Hz  {} ch",
        samples.len() / channels as usize, sample_rate, channels
    );
    Ok(Pcm { sample_rate, channels, samples })
}

/// Decode an Opus track via the `audiopus` binding (libopus is already
/// linked in for Opus encoding — see `encode_opus` below), since the
/// bundled symphonia has never shipped a native Opus decoder. Handles the
/// RFC 7845 (Ogg Opus) framing that libopus's raw decoder doesn't do on its
/// own: reading pre-skip/gain from the OpusHead header packet, skipping the
/// OpusTags comment packet, trimming the pre-skip priming samples, and
/// applying output gain. Returns (sample_rate, channels).
fn decode_opus_via_audiopus(
    format: &mut dyn symphonia::core::formats::FormatReader,
    track_id: u32,
    codec_params: &symphonia::core::codecs::audio::AudioCodecParameters,
    out: &mut Vec<f32>,
) -> Result<(u32, u32)> {
    use audiopus::{coder::Decoder as OpusDecoder, Channels, SampleRate};

    // RFC 7845 §5.1: magic(8) version(1) channels(1) pre_skip_u16le(2)
    // input_sample_rate_u32le(4) output_gain_i16le(2) map_family(1) ...
    // Symphonia's Ogg demuxer stashes the raw header packet here.
    let head = codec_params.extra_data.as_deref()
        .context("Opus: missing OpusHead header (extra_data)")?;
    if head.len() < 19 || &head[0..8] != b"OpusHead" {
        bail!("Opus: malformed or missing OpusHead header");
    }
    let out_channels    = (head[9] as u32).max(1);
    let mut pre_skip    = u16::from_le_bytes([head[10], head[11]]) as usize;
    let output_gain_q78 = i16::from_le_bytes([head[16], head[17]]);
    let gain = 10f32.powf(output_gain_q78 as f32 / 256.0 / 20.0);

    // Opus always decodes at 48 kHz internally, regardless of the original
    // input_sample_rate recorded in OpusHead.
    let channels_enum = if out_channels == 1 { Channels::Mono } else { Channels::Stereo };
    let mut decoder = OpusDecoder::new(SampleRate::Hz48000, channels_enum)
        .context("Opus decoder init")?;

    let ch = out_channels as usize;
    let mut pcm = vec![0f32; 5760 * ch]; // max Opus frame: 120 ms @ 48 kHz

    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None)    => break,
            Err(e)      => { log::warn!("packet read: {e}"); break; }
        };
        if packet.track_id != track_id { continue; }
        if packet.data.starts_with(b"OpusTags") { continue; } // comment packet, not audio

        let n = match decoder.decode_float(Some(packet.data.as_ref()), &mut pcm, false) {
            Ok(n) => n,
            Err(e) => { log::warn!("Opus decode error (continuing): {e:?}"); continue; }
        };
        let frame = &pcm[..n * ch];
        if pre_skip > 0 {
            let skip = pre_skip.min(n);
            pre_skip -= skip;
            out.extend_from_slice(&frame[skip * ch..]);
        } else {
            out.extend_from_slice(frame);
        }
    }

    if gain != 1.0 {
        for s in out.iter_mut() { *s *= gain; }
    }

    Ok((48_000, out_channels))
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
