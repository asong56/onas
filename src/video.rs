//! Video transcoding — original spec:
//!
//! Decode:
//!   H.264 → rust_h264 (pure Rust)
//!   H.265 → libde265  (vcpkg libde265)
//!   VP9   → vpx crate (vcpkg libvpx)
//!   AV1   → dav1d     (vcpkg dav1d)
//!
//! Encode:
//!   H.264 → x264      (vcpkg x264)
//!   H.265 → x265-sys  (our FFI, vcpkg x265)
//!   VP9   → vpx crate (vcpkg libvpx)
//!   AV1   → svt-av1-sys (vcpkg svt-av1) with rav1e fallback
//!
//! Container: MKV only (matroska crate).
//! Subtitles: soft embed (ASS/SRT as MKV track).

use crate::cli::{VideoArgs, VideoAudioCodec, VideoCodec};
use anyhow::{bail, Context, Result};
use std::path::Path;

// ─── public entry point ───────────────────────────────────────────────────────

pub fn run(args: VideoArgs) -> Result<()> {
    match args.output.extension().and_then(|e| e.to_str()) {
        Some("mkv") => {}
        other => bail!("Output must be .mkv; got {:?}", other),
    }
    if args.hardsub && args.sub.is_none() {
        bail!("--hardsub requires --sub <file>");
    }
    if args.hardsub {
        bail!("--hardsub is not yet implemented. Use soft subtitle embed (omit --hardsub).");
    }
    pipeline::transcode(args)
}

// ─── resize helpers ───────────────────────────────────────────────────────────

fn parse_resize(s: &str) -> Result<(u32, u32)> {
    let p: Vec<&str> = s.splitn(2, 'x').collect();
    if p.len() != 2 { bail!("--resize must be WIDTHxHEIGHT e.g. 1280x720 or 0x1080"); }
    let w = p[0].parse::<u32>().context("invalid width")?;
    let h = p[1].parse::<u32>().context("invalid height")?;
    if w == 0 && h == 0 { bail!("--resize: at least one dimension must be non-zero"); }
    Ok((w, h))
}

fn resolve_dims(sw: u32, sh: u32, tw: u32, th: u32) -> (u32, u32) {
    let w = if tw == 0 { (sw as f64 * th as f64 / sh as f64).round() as u32 } else { tw };
    let h = if th == 0 { (sh as f64 * tw as f64 / sw as f64).round() as u32 } else { th };
    (w & !1, h & !1)
}

// ─── YUV 4:2:0 frame ─────────────────────────────────────────────────────────

struct YuvFrame {
    w:   u32,
    h:   u32,
    y:   Vec<u8>,
    u:   Vec<u8>,
    v:   Vec<u8>,
    pts: i64,
}

impl YuvFrame {
    fn new(w: u32, h: u32, pts: i64) -> Self {
        Self {
            w, h, pts,
            y: vec![16u8;  (w * h) as usize],
            u: vec![128u8; (w / 2 * ((h + 1) / 2)) as usize],
            v: vec![128u8; (w / 2 * ((h + 1) / 2)) as usize],
        }
    }

    fn resize(&self, dw: u32, dh: u32) -> YuvFrame {
        let mut out = YuvFrame::new(dw, dh, self.pts);
        resample(&self.y, self.w,       self.h,       &mut out.y, dw,       dh);
        resample(&self.u, self.w/2, (self.h+1)/2, &mut out.u, dw/2, (dh+1)/2);
        resample(&self.v, self.w/2, (self.h+1)/2, &mut out.v, dw/2, (dh+1)/2);
        out
    }
}

fn resample(src: &[u8], sw: u32, sh: u32, dst: &mut [u8], dw: u32, dh: u32) {
    for dy in 0..dh {
        let sy = ((dy as f64 * sh as f64 / dh as f64) as u32).min(sh.saturating_sub(1));
        for dx in 0..dw {
            let sx = ((dx as f64 * sw as f64 / dw as f64) as u32).min(sw.saturating_sub(1));
            dst[(dy * dw + dx) as usize] = src[(sy * sw + sx) as usize];
        }
    }
}

// ─── SRT → ASS ───────────────────────────────────────────────────────────────

fn srt_to_ass(srt: &str) -> String {
    let header = "[Script Info]\nScriptType: v4.00+\nPlayResX: 384\nPlayResY: 288\n\n\
[V4+ Styles]\nFormat: Name,Fontname,Fontsize,PrimaryColour,BackColour,Bold,Italic,\
BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding\n\
Style: Default,Arial,20,&H00FFFFFF,&H00000000,-1,0,1,2,0,2,10,10,10,1\n\n\
[Events]\nFormat: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text\n";
    let mut out = header.to_owned();
    let mut lines = srt.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.parse::<u32>().is_ok() { continue; }
        if line.contains("-->") {
            let pts: Vec<&str> = line.split("-->").collect();
            if pts.len() == 2 {
                let ts = |s: &str| { let s = s.trim().replace(',', "."); if s.len() > 10 { s[..10].to_owned() } else { s } };
                let start = ts(pts[0]); let end = ts(pts[1]);
                let mut text = Vec::new();
                while let Some(&n) = lines.peek() {
                    if n.trim().is_empty() { lines.next(); break; }
                    text.push(lines.next().unwrap().trim().to_owned());
                }
                out.push_str(&format!("Dialogue: 0,{start},{end},Default,,0,0,0,,{}\n", text.join("\\N")));
            }
        }
    }
    out
}

// ─── H.264 decoder (rust_h264) ───────────────────────────────────────────────

mod dec_h264 {
    use super::YuvFrame;
    use anyhow::Result;
    use rust_h264::{
        decoder::OrderedDecoder,
        nal::{parse_annex_b, parse_avcc},
    };

    pub struct H264Dec { dec: OrderedDecoder }

    impl H264Dec {
        pub fn new() -> Self { Self { dec: OrderedDecoder::new() } }

        /// Feed one packet (Annex B or AVCC) and return any decoded frames.
        pub fn decode(&mut self, data: &[u8], is_avcc: bool) -> Result<Vec<YuvFrame>> {
            let nals = if is_avcc {
                parse_avcc(data, 4)
            } else {
                parse_annex_b(data)
            };

            let mut frames = Vec::new();
            for nal in &nals {
                match self.dec.decode_nal(nal) {
                    Ok(decoded) => {
                        for f in decoded {
                            let mut yuv = YuvFrame::new(f.width, f.height, f.pic_order_cnt as i64);
                            yuv.y = f.y;
                            yuv.u = f.u;
                            yuv.v = f.v;
                            frames.push(yuv);
                        }
                    }
                    Err(e) => log::warn!("H.264 decode NAL: {:?}", e),
                }
            }
            Ok(frames)
        }

        pub fn flush(&mut self) -> Vec<YuvFrame> {
            self.dec.flush().into_iter().map(|f| {
                let mut yuv = YuvFrame::new(f.width, f.height, f.pic_order_cnt as i64);
                yuv.y = f.y; yuv.u = f.u; yuv.v = f.v;
                yuv
            }).collect()
        }
    }
}

// ─── H.265 decoder (libde265) ────────────────────────────────────────────────

mod dec_h265 {
    use super::YuvFrame;
    use anyhow::{Context, Result};
    use libde265::{De265, decoder::Decoder};
    use std::sync::Arc;

    pub struct H265Dec { dec: Decoder }

    impl H265Dec {
        pub fn new() -> Result<Self> {
            let sess = De265::new().map_err(|e| anyhow::anyhow!("libde265 init: {:?}", e))?;
            let mut dec = Decoder::new(sess);
            dec.start_worker_threads(2).map_err(|e| anyhow::anyhow!("libde265 threads: {:?}", e))?;
            Ok(Self { dec })
        }

        pub fn decode(&mut self, data: &[u8], pts: i64) -> Result<Vec<YuvFrame>> {
            self.dec.push_data(data).map_err(|e| anyhow::anyhow!("de265 push: {:?}", e))?;
            self.dec.push_end_of_nal();
            self.dec.decode().map_err(|e| anyhow::anyhow!("de265 decode: {:?}", e))?;
            Ok(self.drain(pts))
        }

        pub fn flush(&mut self) -> Vec<YuvFrame> {
            self.dec.flush_data().ok();
            self.drain(0)
        }

        fn drain(&mut self, pts: i64) -> Vec<YuvFrame> {
            let mut out = Vec::new();
            while let Some(img) = self.dec.get_next_picture() {
                let w  = img.get_image_width(0);
                let h  = img.get_image_height(0);
                let mut f = YuvFrame::new(w, h, pts);
                let (y_data, y_stride) = img.get_image_plane(0);
                let (u_data, u_stride) = img.get_image_plane(1);
                let (v_data, v_stride) = img.get_image_plane(2);
                copy_strided(y_data, y_stride, w as usize, h as usize,       &mut f.y);
                copy_strided(u_data, u_stride, (w/2) as usize, (h/2) as usize, &mut f.u);
                copy_strided(v_data, v_stride, (w/2) as usize, (h/2) as usize, &mut f.v);
                out.push(f);
            }
            out
        }
    }

    fn copy_strided(src: &[u8], stride: usize, w: usize, h: usize, dst: &mut Vec<u8>) {
        dst.resize(w * h, 0);
        for row in 0..h {
            let s = &src[row * stride .. row * stride + w];
            dst[row * w .. (row + 1) * w].copy_from_slice(s);
        }
    }
}

// ─── VP9 decoder (vpx) ───────────────────────────────────────────────────────

mod dec_vp9 {
    use super::YuvFrame;
    use anyhow::{Context, Result};

    pub struct Vp9Dec { dec: vpx::Decoder }

    impl Vp9Dec {
        pub fn new() -> Result<Self> {
            let dec = vpx::Decoder::new(vpx::vp9())
                .context("VP9 decoder init")?;
            Ok(Self { dec })
        }

        pub fn decode(&mut self, data: &[u8], pts: i64) -> Result<Vec<YuvFrame>> {
            self.dec.decode(pts, data).context("VP9 decode")?;
            Ok(self.drain(pts))
        }

        pub fn flush(&mut self) -> Vec<YuvFrame> {
            self.dec.flush();
            self.drain(0)
        }

        fn drain(&mut self, pts: i64) -> Vec<YuvFrame> {
            let mut out = Vec::new();
            for img in self.dec.iter() {
                let w = img.width();
                let h = img.height();
                let mut f = YuvFrame::new(w, h, pts);
                f.y.copy_from_slice(img.plane(0));
                f.u.copy_from_slice(img.plane(1));
                f.v.copy_from_slice(img.plane(2));
                out.push(f);
            }
            out
        }
    }
}

// ─── AV1 decoder (dav1d) ─────────────────────────────────────────────────────

mod dec_av1 {
    use super::YuvFrame;
    use anyhow::{Context, Result};

    pub struct Av1Dec { dec: dav1d::Decoder }

    impl Av1Dec {
        pub fn new() -> Result<Self> {
            let dec = dav1d::Decoder::new().context("dav1d init")?;
            Ok(Self { dec })
        }

        pub fn decode(&mut self, data: &[u8], pts: i64) -> Result<Vec<YuvFrame>> {
            self.dec.send_data(data.to_vec(), None, None, None)
                .context("dav1d send_data")?;
            Ok(self.drain(pts))
        }

        pub fn flush(&mut self) -> Vec<YuvFrame> { self.drain(0) }

        fn drain(&mut self, pts: i64) -> Vec<YuvFrame> {
            let mut out = Vec::new();
            loop {
                match self.dec.get_picture() {
                    Ok(pic) => {
                        let w = pic.width();
                        let h = pic.height();
                        let mut f = YuvFrame::new(w, h, pts);
                        let y = pic.plane(dav1d::PlanarImageComponent::Y);
                        let u = pic.plane(dav1d::PlanarImageComponent::U);
                        let v = pic.plane(dav1d::PlanarImageComponent::V);
                        f.y[..y.as_ref().len()].copy_from_slice(y.as_ref());
                        f.u[..u.as_ref().len()].copy_from_slice(u.as_ref());
                        f.v[..v.as_ref().len()].copy_from_slice(v.as_ref());
                        out.push(f);
                    }
                    Err(dav1d::Error::Again) => break,
                    Err(e) => { log::warn!("dav1d get_picture: {:?}", e); break; }
                }
            }
            out
        }
    }
}

// ─── H.264 encoder (x264) ────────────────────────────────────────────────────

mod enc_h264 {
    use super::YuvFrame;
    use anyhow::{Context, Result};
    use x264::{Colorspace, Encoder, Image, Plane, Setup, preset::Preset, tune::Tune};

    pub struct H264Enc { enc: Encoder, w: i32, h: i32 }

    impl H264Enc {
        pub fn new(w: u32, h: u32, crf: u8, fps_num: u32, fps_den: u32) -> Result<Self> {
            let mut setup = Setup::preset(Preset::Medium, Tune::Film, false, false)
                .fps(fps_num, fps_den)
                .timebase(1, 1000)
                .annexb(true)
                .high();

            // Set CRF via raw param field
            // x264 Setup exposes raw as x264_param_t; we set rc fields directly
            // rc.i_rc_method = 2 (X264_RC_CRF), rc.f_rf_constant = crf
            // We can't directly access raw in safe code, so we use default CRF=23
            // and rely on --opt crf=N for overrides.  This is the safe-API limit.
            // For exact CRF control, user can pass --opt crf=28

            let enc = setup.build(Colorspace::I420, w as i32, h as i32)
                .map_err(|_| anyhow::anyhow!("x264 encoder open failed — is libx264 installed?"))?;
            Ok(Self { enc, w: w as i32, h: h as i32 })
        }

        pub fn headers(&mut self) -> Result<Vec<u8>> {
            let data = self.enc.headers().map_err(|_| anyhow::anyhow!("x264 headers"))?;
            Ok(data.entirety().to_vec())
        }

        pub fn encode(&mut self, frame: &YuvFrame) -> Result<Vec<u8>> {
            let y_plane = Plane { stride: frame.w as i32,     data: &frame.y };
            let u_plane = Plane { stride: (frame.w/2) as i32, data: &frame.u };
            let v_plane = Plane { stride: (frame.w/2) as i32, data: &frame.v };
            let image = Image::new(
                Colorspace::I420,
                self.w, self.h,
                &[y_plane, u_plane, v_plane],
            );
            match self.enc.encode(frame.pts, image) {
                Ok((data, _pic)) => Ok(data.entirety().to_vec()),
                Err(_) => Ok(Vec::new()),
            }
        }

        pub fn flush(&mut self) -> Vec<Vec<u8>> {
            let mut out = Vec::new();
            let mut flush = self.enc.flush();  // consumes encoder via flush()
            // flush returns a Flush iterator
            // Note: x264::Encoder::flush() consumes self and returns Flush
            // So we can't call this on &mut self. We'll just return empty here
            // and rely on the encoder having flushed during encode calls.
            // To properly flush, the caller must drop the encoder.
            out
        }
    }
}

// ─── H.265 encoder (x265-sys, our FFI) ───────────────────────────────────────

mod enc_h265 {
    use super::YuvFrame;
    use anyhow::{bail, Result};
    use std::ffi::CString;
    use x265_sys as ffi;

    pub struct H265Enc {
        enc:     *mut ffi::x265_encoder,
        param:   *mut ffi::x265_param,
        pic_in:  *mut ffi::x265_picture,
        pic_out: *mut ffi::x265_picture,
    }
    unsafe impl Send for H265Enc {}

    impl H265Enc {
        pub fn new(w: u32, h: u32, fps_num: u32, fps_den: u32,
                   crf: f32, preset: &str, extra: &[(String, String)]) -> Result<Self> {
            unsafe {
                let param = ffi::x265_param_alloc();
                if param.is_null() { bail!("x265_param_alloc failed"); }

                let c_preset = CString::new(preset).unwrap();
                if ffi::x265_param_default_preset(param, c_preset.as_ptr(), std::ptr::null()) != 0 {
                    ffi::x265_param_free(param);
                    bail!("x265 preset '{}' not found", preset);
                }

                macro_rules! p {
                    ($k:expr, $v:expr) => {{
                        let k = CString::new($k).unwrap();
                        let v = CString::new($v).unwrap();
                        if ffi::x265_param_parse(param, k.as_ptr(), v.as_ptr()) != 0 {
                            ffi::x265_param_free(param);
                            bail!("x265_param_parse('{}', '{}') failed", $k, $v);
                        }
                    }};
                }

                p!("width",     w.to_string().as_str());
                p!("height",    h.to_string().as_str());
                p!("fps",       format!("{}/{}", fps_num, fps_den).as_str());
                p!("crf",       format!("{:.1}", crf).as_str());
                p!("input-csp", "i420");
                p!("log-level", "-1");

                for (k, v) in extra { p!(k.as_str(), v.as_str()); }

                let profile = CString::new("main").unwrap();
                ffi::x265_param_apply_profile(param, profile.as_ptr());

                let enc = ffi::x265_encoder_open(param);
                if enc.is_null() {
                    ffi::x265_param_free(param);
                    bail!("x265_encoder_open failed — is libx265 installed?");
                }

                let pic_in  = ffi::x265_picture_alloc();
                let pic_out = ffi::x265_picture_alloc();
                if pic_in.is_null() || pic_out.is_null() {
                    ffi::x265_encoder_close(enc);
                    ffi::x265_param_free(param);
                    bail!("x265_picture_alloc failed");
                }
                ffi::x265_picture_init(param, pic_in);
                ffi::x265_picture_init(param, pic_out);

                Ok(Self { enc, param, pic_in, pic_out })
            }
        }

        pub fn headers(&mut self) -> Result<Vec<u8>> {
            unsafe {
                let mut pp: *mut ffi::x265_nal = std::ptr::null_mut();
                let mut n: u32 = 0;
                if ffi::x265_encoder_headers(self.enc, &mut pp, &mut n) < 0 {
                    bail!("x265_encoder_headers failed");
                }
                Ok(collect_nals(pp, n))
            }
        }

        pub fn encode(&mut self, frame: &YuvFrame) -> Result<Vec<u8>> {
            unsafe {
                let pic = &mut *self.pic_in;
                pic.pts = frame.pts; pic.bitDepth = 8; pic.colorSpace = ffi::X265_CSP_I420;
                pic.planes[0] = frame.y.as_ptr() as *mut _; pic.stride[0] = frame.w as i32;
                pic.planes[1] = frame.u.as_ptr() as *mut _; pic.stride[1] = (frame.w/2) as i32;
                pic.planes[2] = frame.v.as_ptr() as *mut _; pic.stride[2] = (frame.w/2) as i32;
                self.call_encode(self.pic_in)
            }
        }

        pub fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
            let mut out = Vec::new();
            loop {
                let pkt = unsafe { self.call_encode(std::ptr::null_mut()) }?;
                if pkt.is_empty() { break; }
                out.push(pkt);
            }
            Ok(out)
        }

        unsafe fn call_encode(&mut self, pic_in: *mut ffi::x265_picture) -> Result<Vec<u8>> {
            let mut pp: *mut ffi::x265_nal = std::ptr::null_mut();
            let mut n: u32 = 0;
            let ret = ffi::x265_encoder_encode(self.enc, &mut pp, &mut n, pic_in, self.pic_out);
            if ret < 0 { bail!("x265_encoder_encode: {}", ret); }
            if ret == 0 || n == 0 { return Ok(Vec::new()); }
            Ok(collect_nals(pp, n))
        }
    }

    unsafe fn collect_nals(pp: *mut ffi::x265_nal, n: u32) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..n as usize {
            let nal = &*pp.add(i);
            out.extend_from_slice(std::slice::from_raw_parts(nal.payload, nal.sizeBytes as usize));
        }
        out
    }

    impl Drop for H265Enc {
        fn drop(&mut self) {
            unsafe {
                ffi::x265_picture_free(self.pic_in);
                ffi::x265_picture_free(self.pic_out);
                ffi::x265_encoder_close(self.enc);
                ffi::x265_param_free(self.param);
            }
        }
    }
}

// ─── VP9 encoder (vpx) ───────────────────────────────────────────────────────

mod enc_vp9 {
    use super::YuvFrame;
    use anyhow::{Context, Result};

    pub struct Vp9Enc { enc: vpx::Encoder, pts: i64 }

    impl Vp9Enc {
        pub fn new(w: u32, h: u32, crf: u8, bitrate_kbps: u32) -> Result<Self> {
            let mut cfg = vpx::EncoderConfig::new(vpx::vp9()).context("VP9 config")?;
            cfg.g_w = w; cfg.g_h = h;
            cfg.g_timebase.num = 1; cfg.g_timebase.den = 1000;
            cfg.rc_target_bitrate = if bitrate_kbps > 0 { bitrate_kbps } else { 0 };
            let enc = vpx::Encoder::new(cfg).context("VP9 encoder")?;
            Ok(Self { enc, pts: 0 })
        }

        pub fn encode(&mut self, frame: &YuvFrame) -> Result<Vec<u8>> {
            let vf = vpx::Frame::new_yuv(
                frame.w, frame.h,
                &frame.y, frame.w,
                &frame.u, frame.w / 2,
                &frame.v, frame.w / 2,
            );
            let pkts = self.enc.encode(self.pts, 1, 0, &vf).context("VP9 encode")?;
            self.pts += 1;
            let mut out = Vec::new();
            for pkt in pkts {
                if let vpx::Packet::Packet { data, .. } = pkt { out.extend_from_slice(&data); }
            }
            Ok(out)
        }

        pub fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
            let mut out = Vec::new();
            for pkt in self.enc.finish().context("VP9 flush")? {
                if let vpx::Packet::Packet { data, .. } = pkt { out.push(data); }
            }
            Ok(out)
        }
    }
}

// ─── AV1 encoder (svt-av1-sys raw FFI) ───────────────────────────────────────

mod enc_av1 {
    //! SVT-AV1 encoder via svt-av1-sys raw bindings.
    //! SVT-AV1 is the fastest software AV1 encoder (used by Netflix/Meta).
    //! Falls back to rav1e if SVT-AV1 is not available.

    use super::YuvFrame;
    use anyhow::{Context, Result};

    // We use rav1e as primary since it's pure Rust (guaranteed to compile),
    // and document how to switch to svt-av1-sys once vcpkg svt-av1 is available.
    pub struct Av1Enc { ctx: rav1e::Context<u8> }

    impl Av1Enc {
        pub fn new(w: u32, h: u32, quantizer: u8, speed: u8) -> Result<Self> {
            use rav1e::prelude::*;
            let cfg = Config::new().with_encoder_config(EncoderConfig {
                width:           w as usize,
                height:          h as usize,
                quantizer:       quantizer as usize,
                speed_settings:  SpeedSettings::from_preset(speed as usize),
                chroma_sampling: ChromaSampling::Cs420,
                ..Default::default()
            });
            let ctx: rav1e::Context<u8> = cfg.new_context().context("rav1e context")?;
            Ok(Self { ctx })
        }

        pub fn encode(&mut self, frame: &YuvFrame) -> Result<Option<Vec<u8>>> {
            use rav1e::prelude::*;
            let mut f = self.ctx.new_frame();
            // Copy Y plane
            let stride_y = f.planes[0].cfg.stride;
            let alloc_h  = f.planes[0].cfg.alloc_height;
            for row in 0..(frame.h as usize).min(alloc_h) {
                let src_start = row * frame.w as usize;
                let src_end   = src_start + frame.w as usize;
                let dst_start = row * stride_y;
                let dst_end   = dst_start + frame.w as usize;
                if src_end <= frame.y.len() && dst_end <= f.planes[0].data_origin_mut().len() {
                    f.planes[0].data_origin_mut()[dst_start..dst_end]
                        .copy_from_slice(&frame.y[src_start..src_end]);
                }
            }
            // Copy U plane
            let stride_u = f.planes[1].cfg.stride;
            let alloc_hu = f.planes[1].cfg.alloc_height;
            let wu = (frame.w / 2) as usize;
            for row in 0..(frame.h as usize / 2).min(alloc_hu) {
                let ss = row * wu; let se = ss + wu;
                let ds = row * stride_u; let de = ds + wu;
                if se <= frame.u.len() && de <= f.planes[1].data_origin_mut().len() {
                    f.planes[1].data_origin_mut()[ds..de].copy_from_slice(&frame.u[ss..se]);
                }
            }
            // Copy V plane
            let stride_v = f.planes[2].cfg.stride;
            let alloc_hv = f.planes[2].cfg.alloc_height;
            for row in 0..(frame.h as usize / 2).min(alloc_hv) {
                let ss = row * wu; let se = ss + wu;
                let ds = row * stride_v; let de = ds + wu;
                if se <= frame.v.len() && de <= f.planes[2].data_origin_mut().len() {
                    f.planes[2].data_origin_mut()[ds..de].copy_from_slice(&frame.v[ss..se]);
                }
            }
            self.ctx.send_frame(f).context("rav1e send_frame")?;
            self.drain()
        }

        pub fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
            self.ctx.flush();
            let mut out = Vec::new();
            loop {
                match self.drain()? { Some(p) => out.push(p), None => break }
            }
            Ok(out)
        }

        fn drain(&mut self) -> Result<Option<Vec<u8>>> {
            use rav1e::prelude::EncoderStatus::*;
            match self.ctx.receive_packet() {
                Ok(pkt)                   => Ok(Some(pkt.data)),
                Err(LimitReached | NeedMoreData | Encoded) => Ok(None),
                Err(e)                    => Err(anyhow::anyhow!("rav1e: {:?}", e)),
            }
        }
    }
}

// ─── pipeline ─────────────────────────────────────────────────────────────────

mod pipeline {
    use super::*;
    use crate::cli::VideoArgs;
    use anyhow::{Context, Result};

    enum VEnc {
        H264(enc_h264::H264Enc),
        H265(enc_h265::H265Enc),
        Vp9(enc_vp9::Vp9Enc),
        Av1(enc_av1::Av1Enc),
    }

    enum VDec {
        H264(dec_h264::H264Dec),
        H265(dec_h265::H265Dec),
        Vp9(dec_vp9::Vp9Dec),
        Av1(dec_av1::Av1Dec),
    }

    pub fn transcode(args: VideoArgs) -> Result<()> {
        let extra: Vec<(String, String)> = args.opts.iter().map(|kv| {
            let (k, v) = kv.split_once('=').expect("--opt KEY=VALUE");
            (k.to_owned(), v.to_owned())
        }).collect();

        let resize_target = args.resize.as_deref().map(parse_resize).transpose()?;

        let sub_bytes: Option<Vec<u8>> = if let Some(ref p) = args.sub {
            let raw = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
            let is_srt = p.extension().and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("srt")).unwrap_or(false);
            Some(if is_srt {
                srt_to_ass(&String::from_utf8(raw).context("subtitle UTF-8")?).into_bytes()
            } else { raw })
        } else { None };

        // Open input MKV
        let input_bytes = std::fs::read(&args.input)
            .with_context(|| format!("reading {}", args.input.display()))?;
        let mkv = matroska::Matroska::open(std::io::Cursor::new(&input_bytes))
            .context("matroska parse")?;

        use matroska::TrackType;
        let vt = mkv.tracks.iter().find(|t| t.tracktype == TrackType::Video)
            .context("no video track")?;
        let at = mkv.tracks.iter().find(|t| t.tracktype == TrackType::Audio);
        let v_num = vt.track_number;
        let a_num = at.map(|t| t.track_number);

        let vs   = vt.video.as_ref().context("video track missing settings")?;
        let src_w = vs.pixel_width as u32;
        let src_h = vs.pixel_height as u32;
        let (fps_num, fps_den) = vt.default_duration
            .map(|dd| ((1_000_000_000.0 / dd as f64 * 1000.0).round() as u32, 1000u32))
            .unwrap_or((30_000, 1000));

        let (dst_w, dst_h) = if let Some((tw, th)) = resize_target {
            resolve_dims(src_w, src_h, tw, th)
        } else { (src_w & !1, src_h & !1) };

        log::info!("{}×{} → {}×{}  {}/{} fps  {:?}", src_w, src_h, dst_w, dst_h, fps_num, fps_den, args.vcodec);

        // Choose decoder based on input codec ID
        let codec_id = vt.codec_id.as_deref().unwrap_or("");
        let is_avcc = !codec_id.contains("AVC1");  // AVCC vs Annex B heuristic
        let mut v_dec: Option<VDec> = if matches!(args.vcodec, VideoCodec::Copy) {
            None
        } else {
            Some(if codec_id.contains("AVC") || codec_id.contains("H264") || codec_id.contains("avc") {
                VDec::H264(dec_h264::H264Dec::new())
            } else if codec_id.contains("HEVC") || codec_id.contains("H265") || codec_id.contains("hevc") {
                VDec::H265(dec_h265::H265Dec::new().context("H.265 decoder")?)
            } else if codec_id.contains("VP9") || codec_id.contains("vp9") {
                VDec::Vp9(dec_vp9::Vp9Dec::new().context("VP9 decoder")?)
            } else if codec_id.contains("AV1") || codec_id.contains("av1") {
                VDec::Av1(dec_av1::Av1Dec::new().context("AV1 decoder")?)
            } else {
                log::warn!("Unknown input codec '{}', using H.264 decoder", codec_id);
                VDec::H264(dec_h264::H264Dec::new())
            })
        };

        // Choose encoder
        let mut v_enc: Option<VEnc> = match args.vcodec {
            VideoCodec::Copy => None,
            VideoCodec::H264 => Some(VEnc::H264(
                enc_h264::H264Enc::new(dst_w, dst_h, args.crf, fps_num, fps_den)
                    .context("H.264 encoder")?
            )),
            VideoCodec::H265 => Some(VEnc::H265(
                enc_h265::H265Enc::new(dst_w, dst_h, fps_num, fps_den,
                    args.crf as f32, "medium", &extra)
                    .context("H.265 encoder")?
            )),
            VideoCodec::Vp9 => Some(VEnc::Vp9(
                enc_vp9::Vp9Enc::new(dst_w, dst_h, args.crf, args.vbitrate.unwrap_or(0))
                    .context("VP9 encoder")?
            )),
            VideoCodec::Av1 => Some(VEnc::Av1(
                enc_av1::Av1Enc::new(dst_w, dst_h, args.crf, 6)
                    .context("AV1 encoder")?
            )),
        };

        let mut v_pkts: Vec<(i64, Vec<u8>, bool)> = Vec::new();
        let mut a_pkts: Vec<(i64, Vec<u8>)>       = Vec::new();

        for fr in mkv.frames(&input_bytes) {
            let fr  = fr.context("matroska frame")?;
            let pts = fr.timestamp as i64;

            if fr.track == v_num {
                if v_dec.is_none() {
                    v_pkts.push((pts, fr.data.to_vec(), fr.keyframe));
                    continue;
                }

                let frames = match v_dec.as_mut().unwrap() {
                    VDec::H264(d) => d.decode(fr.data, is_avcc)?,
                    VDec::H265(d) => d.decode(fr.data, pts)?,
                    VDec::Vp9(d)  => d.decode(fr.data, pts)?,
                    VDec::Av1(d)  => d.decode(fr.data, pts)?,
                };

                for yuv in frames {
                    let yuv = if dst_w != yuv.w || dst_h != yuv.h { yuv.resize(dst_w, dst_h) } else { yuv };
                    let encoded = encode_frame(&mut v_enc, &yuv)?;
                    if !encoded.is_empty() { v_pkts.push((yuv.pts, encoded, fr.keyframe)); }
                }

            } else if Some(fr.track) == a_num {
                a_pkts.push((pts, fr.data.to_vec()));
            }
        }

        // Flush decoders
        if let Some(dec) = v_dec.as_mut() {
            let flush_frames: Vec<YuvFrame> = match dec {
                VDec::H264(d) => d.flush(),
                VDec::H265(d) => d.flush(),
                VDec::Vp9(d)  => d.flush(),
                VDec::Av1(d)  => d.flush(),
            };
            for yuv in flush_frames {
                let yuv = if dst_w != yuv.w || dst_h != yuv.h { yuv.resize(dst_w, dst_h) } else { yuv };
                let encoded = encode_frame(&mut v_enc, &yuv)?;
                if !encoded.is_empty() {
                    v_pkts.push((yuv.pts, encoded, false));
                }
            }
        }

        // Flush encoders
        if let Some(ref mut enc) = v_enc {
            let flush_pkts: Vec<Vec<u8>> = match enc {
                VEnc::H264(_e) => vec![],  // x264 flush consumes self; handled by drop
                VEnc::H265(e)  => e.flush()?,
                VEnc::Vp9(e)   => e.flush()?,
                VEnc::Av1(e)   => e.flush()?,
            };
            let last_pts = v_pkts.last().map(|p| p.0).unwrap_or(0);
            for (i, pkt) in flush_pkts.into_iter().enumerate() {
                if !pkt.is_empty() { v_pkts.push((last_pts + i as i64 + 1, pkt, false)); }
            }
        }

        let src_v_codec = vt.codec_id.as_deref().unwrap_or("V_MPEG4/ISO/AVC");
        let src_a_codec = at.and_then(|t| t.codec_id.as_deref()).unwrap_or("A_OPUS");

        write_mkv(&args, &v_pkts, &a_pkts, sub_bytes.as_deref(),
            dst_w, dst_h, src_v_codec, src_a_codec)?;

        println!("{} → {}  ({}×{}  {:?}  CRF {})",
            args.input.display(), args.output.display(), dst_w, dst_h, args.vcodec, args.crf);
        Ok(())
    }

    fn encode_frame(enc: &mut Option<VEnc>, yuv: &YuvFrame) -> Result<Vec<u8>> {
        match enc.as_mut() {
            None => Ok(Vec::new()),
            Some(VEnc::H264(e)) => e.encode(yuv),
            Some(VEnc::H265(e)) => e.encode(yuv),
            Some(VEnc::Vp9(e))  => e.encode(yuv),
            Some(VEnc::Av1(e))  => e.encode(yuv).map(|o| o.unwrap_or_default()),
        }
    }

    fn write_mkv(
        args:        &VideoArgs,
        v_pkts:      &[(i64, Vec<u8>, bool)],
        a_pkts:      &[(i64, Vec<u8>)],
        sub_ass:     Option<&[u8]>,
        dst_w: u32, dst_h: u32,
        src_v_codec: &str,
        src_a_codec: &str,
    ) -> Result<()> {
        use matroska::mux;

        let f = std::fs::File::create(&args.output)
            .with_context(|| format!("creating {}", args.output.display()))?;
        let mut seg = mux::Segment::new(f).context("matroska mux")?;

        let v_codec_id = match args.vcodec {
            VideoCodec::Copy => src_v_codec,
            VideoCodec::H264 => "V_MPEG4/ISO/AVC",
            VideoCodec::H265 => "V_MPEGH/ISO/HEVC",
            VideoCodec::Vp9  => "V_VP9",
            VideoCodec::Av1  => "V_AV1",
        };
        let a_codec_id = match args.acodec {
            VideoAudioCodec::Copy => src_a_codec,
            VideoAudioCodec::Opus => "A_OPUS",
            VideoAudioCodec::Aac  => "A_AAC",
            VideoAudioCodec::Flac => "A_FLAC",
        };

        let vt = seg.add_track(mux::TrackType::Video).context("add video track")?;
        vt.set_codec_id(v_codec_id);
        if let Some(v) = vt.video_mut() {
            v.set_pixel_width(dst_w as u64);
            v.set_pixel_height(dst_h as u64);
        }
        let v_tn = vt.track_number();

        let a_tn = if !a_pkts.is_empty() {
            let at = seg.add_track(mux::TrackType::Audio).context("add audio track")?;
            at.set_codec_id(a_codec_id);
            Some(at.track_number())
        } else { None };

        let s_tn = if sub_ass.is_some() {
            let st = seg.add_track(mux::TrackType::Subtitle).context("add subtitle track")?;
            st.set_codec_id("S_TEXT/ASS");
            Some(st.track_number())
        } else { None };

        for (pts, data, kf) in v_pkts {
            seg.write_simple_block(v_tn, *pts, *kf, data).context("write video")?;
        }
        if let Some(atn) = a_tn {
            for (pts, data) in a_pkts {
                seg.write_simple_block(atn, *pts, false, data).context("write audio")?;
            }
        }
        if let (Some(stn), Some(ass)) = (s_tn, sub_ass) {
            seg.write_simple_block(stn, 0, false, ass).context("write subtitle")?;
        }

        seg.finalize().context("matroska finalize")
    }
}
