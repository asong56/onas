//! Video transcoding.
//!
//! Decode: H.264 → rust_h264 | H.265 → libde265 | VP9 → vpx | AV1 → dav1d
//! Encode: H.264 → x264 | H.265 → x265-sys | VP9 → vpx | AV1 → rav1e
//! Container: MKV read via matroska (metadata) + webm-iterable (frames)
//!            MKV write via webm-iterable

use crate::cli::{VideoArgs, VideoAudioCodec, VideoCodec};
use anyhow::{bail, Context, Result};

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

// ─── resize helpers ──────────────────────────────────────────────────────────

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
        resample(&self.y, self.w,         self.h,         &mut out.y, dw,       dh);
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
                let ts = |s: &str| {
                    let s = s.trim().replace(',', ".");
                    if s.len() > 10 { s[..10].to_owned() } else { s }
                };
                let start = ts(pts[0]);
                let end   = ts(pts[1]);
                let mut text = Vec::new();
                while let Some(&n) = lines.peek() {
                    if n.trim().is_empty() { lines.next(); break; }
                    text.push(lines.next().unwrap().trim().to_owned());
                }
                out.push_str(&format!("Dialogue: 0,{start},{end},Default,,0,0,0,,{}\n",
                    text.join("\\N")));
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

        pub fn decode(&mut self, data: &[u8], is_avcc: bool) -> Result<Vec<YuvFrame>> {
            let nals = if is_avcc { parse_avcc(data, 4) } else { parse_annex_b(data) };
            let mut frames = Vec::new();
            for nal in &nals {
                match self.dec.decode_nal(nal) {
                    Ok(decoded) => {
                        for f in decoded {
                            let mut yuv = YuvFrame::new(f.width, f.height, f.pic_order_cnt as i64);
                            yuv.y = f.y; yuv.u = f.u; yuv.v = f.v;
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
    use anyhow::Result;
    use libde265::{De265, Decoder};

    pub struct H265Dec { dec: Decoder }

    impl H265Dec {
        pub fn new() -> Result<Self> {
            let sess = De265::new().map_err(|e| anyhow::anyhow!("libde265 init: {:?}", e))?;
            let mut dec = Decoder::new(sess);
            dec.start_worker_threads(2)
                .map_err(|e| anyhow::anyhow!("libde265 threads: {:?}", e))?;
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
                let w = img.get_image_width(0);
                let h = img.get_image_height(0);
                let mut f = YuvFrame::new(w, h, pts);
                let (y_data, y_stride) = img.get_image_plane(0);
                let (u_data, u_stride) = img.get_image_plane(1);
                let (v_data, v_stride) = img.get_image_plane(2);
                copy_strided(y_data, y_stride, w as usize, h as usize,         &mut f.y);
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
            dst[row * w..(row + 1) * w]
                .copy_from_slice(&src[row * stride..row * stride + w]);
        }
    }
}

// ─── VP9 decoder (vpx) ───────────────────────────────────────────────────────

mod dec_vp9 {
    use super::YuvFrame;
    use anyhow::Result;
    use vpx_sys as ffi;
    use std::ptr;

    pub struct Vp9Dec {
        ctx: ffi::vpx_codec_ctx_t,
    }
    unsafe impl Send for Vp9Dec {}

    impl Vp9Dec {
        pub fn new() -> Result<Self> {
            let mut ctx: ffi::vpx_codec_ctx_t = Default::default();
            let err = unsafe {
                ffi::vpx_codec_dec_init_ver(
                    &mut ctx,
                    &mut ffi::vpx_codec_vp9_dx_algo as *mut _,
                    ptr::null(),
                    0,
                    ffi::VPX_DECODER_ABI_VERSION as i32,
                )
            };
            if err != 0 {
                bail_vpx(err, "VP9 decoder init")?;
            }
            Ok(Self { ctx })
        }

        pub fn decode(&mut self, data: &[u8], pts: i64) -> Result<Vec<YuvFrame>> {
            let err = unsafe {
                ffi::vpx_codec_decode(
                    &mut self.ctx,
                    data.as_ptr(),
                    data.len() as u32,
                    ptr::null_mut(),
                    0,
                )
            };
            if err != 0 { bail_vpx(err, "VP9 decode")?; }
            Ok(self.drain(pts))
        }

        pub fn flush(&mut self) -> Vec<YuvFrame> { self.drain(0) }

        fn drain(&mut self, pts: i64) -> Vec<YuvFrame> {
            let mut out = Vec::new();
            let mut iter: ffi::vpx_codec_iter_t = ptr::null();
            loop {
                let img = unsafe {
                    ffi::vpx_codec_get_frame(&mut self.ctx, &mut iter)
                };
                if img.is_null() { break; }
                let img = unsafe { &*img };
                let w = img.d_w;
                let h = img.d_h;
                let mut f = YuvFrame::new(w, h, pts);
                unsafe {
                    let planes = [img.planes[0], img.planes[1], img.planes[2]];
                    let strides = [img.stride[0] as usize, img.stride[1] as usize, img.stride[2] as usize];
                    let (dsts, widths, heights) = (
                        [&mut f.y, &mut f.u, &mut f.v],
                        [w as usize, (w/2) as usize, (w/2) as usize],
                        [h as usize, (h/2) as usize, (h/2) as usize],
                    );
                    for p in 0..3usize {
                        for row in 0..heights[p] {
                            let src = std::slice::from_raw_parts(
                                planes[p].add(row * strides[p]),
                                widths[p],
                            );
                            dsts[p][row * widths[p]..(row + 1) * widths[p]].copy_from_slice(src);
                        }
                    }
                }
                out.push(f);
            }
            out
        }
    }

    impl Drop for Vp9Dec {
        fn drop(&mut self) {
            unsafe { ffi::vpx_codec_destroy(&mut self.ctx); }
        }
    }

    fn bail_vpx(err: ffi::vpx_codec_err_t, ctx: &str) -> Result<()> {
        Err(anyhow::anyhow!("{}: vpx error code {}", ctx, err))
    }
}

// ─── AV1 decoder (dav1d) ─────────────────────────────────────────────────────

mod dec_av1 {
    use super::YuvFrame;
    use anyhow::{Context, Result};

    pub struct Av1Dec { dec: dav1d::Decoder }

    impl Av1Dec {
        pub fn new() -> Result<Self> {
            Ok(Self { dec: dav1d::Decoder::new().context("dav1d init")? })
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
    use anyhow::Result;
    use x264::{Colorspace, Encoder, Image, Plane, Preset, Setup, Tune};

    pub struct H264Enc {
        enc: Option<Encoder>,
        w: i32,
        h: i32,
    }

    impl H264Enc {
        pub fn new(w: u32, h: u32, _crf: u8, fps_num: u32, fps_den: u32) -> Result<Self> {
            let setup = Setup::preset(Preset::Medium, Tune::Film, false, false)
                .fps(fps_num, fps_den)
                .timebase(1, 1000)
                .annexb(true)
                .high();

            let enc = setup.build(Colorspace::I420, w as i32, h as i32)
                .map_err(|_| anyhow::anyhow!(
                    "x264 encoder open failed — is libx264 installed via vcpkg?"
                ))?;
            Ok(Self { enc: Some(enc), w: w as i32, h: h as i32 })
        }

        pub fn headers(&mut self) -> Result<Vec<u8>> {
            let enc = self.enc.as_mut().context("encoder already flushed")?;
            Ok(enc.headers().map_err(|_| anyhow::anyhow!("x264 headers"))?.entirety().to_vec())
        }

        pub fn encode(&mut self, frame: &YuvFrame) -> Result<Vec<u8>> {
            let enc = self.enc.as_mut().context("encoder already flushed")?;
            let y = Plane { stride: frame.w as i32,     data: &frame.y };
            let u = Plane { stride: (frame.w/2) as i32, data: &frame.u };
            let v = Plane { stride: (frame.w/2) as i32, data: &frame.v };
            let image = Image::new(Colorspace::I420, self.w, self.h, &[y, u, v]);
            match enc.encode(frame.pts, image) {
                Ok((data, _)) => Ok(data.entirety().to_vec()),
                Err(_)        => Ok(Vec::new()),
            }
        }

        pub fn flush(&mut self) -> Vec<Vec<u8>> {
            // x264::Encoder::flush() consumes self; take ownership and drain
            if let Some(enc) = self.enc.take() {
                enc.flush()
                    .filter_map(|r| r.ok())
                    .map(|(data, _)| data.entirety().to_vec())
                    .filter(|v| !v.is_empty())
                    .collect()
            } else {
                Vec::new()
            }
        }
    }

}

// ─── H.265 encoder (x265-sys) ────────────────────────────────────────────────

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
                if ffi::x265_param_default_preset(param, c_preset.as_ptr(),
                                                   std::ptr::null()) != 0 {
                    ffi::x265_param_free(param);
                    bail!("x265 preset '{}' not found", preset);
                }

                macro_rules! p {
                    ($k:expr, $v:expr) => {{
                        let k = CString::new($k).unwrap();
                        let v = CString::new($v).unwrap();
                        if ffi::x265_param_parse(param, k.as_ptr(), v.as_ptr()) != 0 {
                            ffi::x265_param_free(param);
                            bail!("x265_param_parse('{}','{}') failed", $k, $v);
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

                ffi::x265_param_apply_profile(param, CString::new("main").unwrap().as_ptr());

                let enc = ffi::x265_encoder_open(param);
                if enc.is_null() {
                    ffi::x265_param_free(param);
                    bail!("x265_encoder_open failed — is libx265 installed via vcpkg?");
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
                pic.pts = frame.pts; pic.bitDepth = 8;
                pic.colorSpace = ffi::X265_CSP_I420;
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
            out.extend_from_slice(
                std::slice::from_raw_parts(nal.payload, nal.sizeBytes as usize)
            );
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

// ─── VP9 encoder (vpx via trait API) ─────────────────────────────────────────

mod enc_vp9 {
    //! VP9 encoder using the vpx crate's trait-based API (commit 04df690).
    //! vpx::encoder::Encoder trait methods: encode(), flush(), packets()
    //! vpx::encoder::vp9::{Interface, Cfg, Context}
    use super::YuvFrame;
    use anyhow::Result;
    use vpx::encoder::{self, Encoder as _, FrameFlags, DL_GOOD_QUALITY};
    use vpx::encoder::vp9 as vp9enc;
    use vpx::{Image, Format, ColorSpace, Interface as _};
    use std::borrow::Cow;

    struct FrameCollector(Vec<Vec<u8>>);
    impl encoder::PacketWriter for FrameCollector {
        fn write_frame<'a>(&mut self, f: &vpx::Frame<'a>) -> std::io::Result<()> {
            self.0.push(f.data().to_vec());
            Ok(())
        }
    }

    pub struct Vp9Enc { ctx: vp9enc::Context, pts: i64 }

    impl Vp9Enc {
        pub fn new(w: u32, h: u32, _crf: u8, bitrate_kbps: u32) -> Result<Self> {
            let mut cfg = vp9enc::Cfg::default();
            cfg.g_w = w;
            cfg.g_h = h;
            cfg.g_timebase.num = 1;
            cfg.g_timebase.den = 1000;
            cfg.rc_target_bitrate = if bitrate_kbps > 0 { bitrate_kbps } else { 200 };
            let iface = vp9enc::Interface::default();
            let ctx = iface.create(cfg, 0)
                .map_err(|e| anyhow::anyhow!("VP9 encoder: {:?}", e))?;
            Ok(Self { ctx, pts: 0 })
        }

        pub fn encode(&mut self, frame: &YuvFrame) -> Result<Vec<u8>> {
            let mut buf = Vec::with_capacity(frame.y.len() + frame.u.len() + frame.v.len());
            buf.extend_from_slice(&frame.y);
            buf.extend_from_slice(&frame.u);
            buf.extend_from_slice(&frame.v);
            let image = Image::new(
                Cow::Owned(buf),
                Format::I420 { hi_bit_depth: false },
                ColorSpace::BT601,
                frame.w, frame.h, frame.w,
            );
            self.ctx.encode(&image, self.pts, 1, FrameFlags::new(), DL_GOOD_QUALITY)
                .map_err(|e| anyhow::anyhow!("VP9 encode: {:?}", e))?;
            self.pts += 1;
            let mut col = FrameCollector(Vec::new());
            self.ctx.packets(&mut col)
                .map_err(|e| anyhow::anyhow!("VP9 packets: {:?}", e))?;
            Ok(col.0.concat())
        }

        pub fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
            self.ctx.flush(self.pts, 1, 0, DL_GOOD_QUALITY)
                .map_err(|e| anyhow::anyhow!("VP9 flush: {:?}", e))?;
            let mut col = FrameCollector(Vec::new());
            self.ctx.packets(&mut col)
                .map_err(|e| anyhow::anyhow!("VP9 flush packets: {:?}", e))?;
            Ok(col.0.into_iter().filter(|v| !v.is_empty()).collect())
        }
    }
}

// ─── AV1 encoder (rav1e) ─────────────────────────────────────────────────────

mod enc_av1 {
    use super::YuvFrame;
    use anyhow::{Context, Result};

    pub struct Av1Enc { ctx: rav1e::Context<u8> }

    impl Av1Enc {
        pub fn new(w: u32, h: u32, quantizer: u8, speed: u8) -> Result<Self> {
            use rav1e::prelude::*;
            let cfg = Config::new().with_encoder_config(EncoderConfig {
                width:           w as usize,
                height:          h as usize,
                quantizer:       quantizer as usize,
                speed_settings:  SpeedSettings::from_preset(speed),
                chroma_sampling: ChromaSampling::Cs420,
                ..Default::default()
            });
            Ok(Self { ctx: cfg.new_context().context("rav1e context")? })
        }

        pub fn encode(&mut self, frame: &YuvFrame) -> Result<Option<Vec<u8>>> {
            use rav1e::prelude::*;
            let mut f = self.ctx.new_frame();
            let copy_plane = |plane: &mut rav1e::Frame<u8>, p: usize, src: &[u8], w: usize, h: usize| {
                let stride = plane.planes[p].cfg.stride;
                let alloc_h = plane.planes[p].cfg.alloc_height;
                for row in 0..h.min(alloc_h) {
                    let ss = row * w; let se = (ss + w).min(src.len());
                    let ds = row * stride; let de = (ds + w).min(plane.planes[p].data_origin_mut().len());
                    if se > ss && de > ds {
                        plane.planes[p].data_origin_mut()[ds..de]
                            .copy_from_slice(&src[ss..ss + (de - ds).min(se - ss)]);
                    }
                }
            };
            copy_plane(&mut f, 0, &frame.y, frame.w as usize,     frame.h as usize);
            copy_plane(&mut f, 1, &frame.u, (frame.w/2) as usize, (frame.h/2) as usize);
            copy_plane(&mut f, 2, &frame.v, (frame.w/2) as usize, (frame.h/2) as usize);
            self.ctx.send_frame(f).context("rav1e send_frame")?;
            self.drain()
        }

        pub fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
            self.ctx.flush();
            let mut out = Vec::new();
            loop { match self.drain()? { Some(p) => out.push(p), None => break } }
            Ok(out)
        }

        fn drain(&mut self) -> Result<Option<Vec<u8>>> {
            use rav1e::prelude::EncoderStatus::*;
            match self.ctx.receive_packet() {
                Ok(pkt)                              => Ok(Some(pkt.data)),
                Err(LimitReached | NeedMoreData | Encoded) => Ok(None),
                Err(e)                               => Err(anyhow::anyhow!("rav1e: {:?}", e)),
            }
        }
    }
}

// ─── pipeline ─────────────────────────────────────────────────────────────────

mod pipeline {
    use super::*;
    use crate::cli::VideoArgs;
    use anyhow::{Context, Result};
    use webm_iterable::{
        WebmIterator, WebmWriter,
        matroska_spec::{MatroskaSpec, SimpleBlock, Master},
    };
    use ebml_iterable::specs::Master as EbmlMaster;

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

    struct TrackInfo {
        v_track: u64,
        a_track: Option<u64>,
        codec_id: String,
        a_codec_id: String,
        src_w: u32,
        src_h: u32,
        fps_num: u32,
        fps_den: u32,
    }

    fn probe_tracks(path: &std::path::Path) -> Result<TrackInfo> {
        use matroska::{Matroska, Tracktype, Settings};
        use std::fs::File;

        let f = File::open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        let mkv = Matroska::open(f).context("matroska probe")?;

        let vt = mkv.tracks.iter()
            .find(|t| t.tracktype == Tracktype::Video)
            .context("no video track in input")?;
        let at = mkv.tracks.iter()
            .find(|t| t.tracktype == Tracktype::Audio);

        let (src_w, src_h) = match &vt.settings {
            Settings::Video(v) => (v.pixel_width as u32, v.pixel_height as u32),
            _ => bail!("video track missing video settings"),
        };

        let fps_num = vt.default_duration
            .map(|ns| ((1_000_000_000_000u64 / ns) as u32))
            .unwrap_or(30);

        Ok(TrackInfo {
            v_track:    vt.number as u64,
            a_track:    at.map(|t| t.number as u64),
            codec_id:   vt.codec_id.clone(),
            a_codec_id: at.map(|t| t.codec_id.clone()).unwrap_or_else(|| "A_OPUS".into()),
            src_w, src_h,
            fps_num, fps_den: 1,
        })
    }

    pub fn transcode(args: VideoArgs) -> Result<()> {
        let extra: Vec<(String, String)> = args.opts.iter().map(|kv| {
            let (k, v) = kv.split_once('=').expect("--opt KEY=VALUE");
            (k.to_owned(), v.to_owned())
        }).collect();

        let resize_target = args.resize.as_deref().map(parse_resize).transpose()?;

        let sub_bytes: Option<Vec<u8>> = if let Some(ref p) = args.sub {
            let raw = std::fs::read(p)
                .with_context(|| format!("reading {}", p.display()))?;
            let is_srt = p.extension().and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("srt")).unwrap_or(false);
            Some(if is_srt {
                srt_to_ass(&String::from_utf8(raw).context("subtitle UTF-8")?).into_bytes()
            } else { raw })
        } else { None };

        let info = probe_tracks(&args.input)?;

        let (dst_w, dst_h) = if let Some((tw, th)) = resize_target {
            resolve_dims(info.src_w, info.src_h, tw, th)
        } else {
            (info.src_w & !1, info.src_h & !1)
        };

        log::info!("{}×{} → {}×{}  {}/{} fps  {:?}",
            info.src_w, info.src_h, dst_w, dst_h,
            info.fps_num, info.fps_den, args.vcodec);

        let is_avcc = !info.codec_id.contains("AVC1");

        let mut v_dec: Option<VDec> = if matches!(args.vcodec, VideoCodec::Copy) {
            None
        } else {
            let cid = &info.codec_id;
            Some(if cid.contains("AVC") || cid.contains("H264") || cid.contains("avc") {
                VDec::H264(dec_h264::H264Dec::new())
            } else if cid.contains("HEVC") || cid.contains("H265") || cid.contains("hevc") {
                VDec::H265(dec_h265::H265Dec::new().context("H.265 decoder")?)
            } else if cid.contains("VP9") || cid.contains("vp9") {
                VDec::Vp9(dec_vp9::Vp9Dec::new().context("VP9 decoder")?)
            } else if cid.contains("AV1") || cid.contains("av1") {
                VDec::Av1(dec_av1::Av1Dec::new().context("AV1 decoder")?)
            } else {
                log::warn!("Unknown codec '{}', trying H.264 decoder", cid);
                VDec::H264(dec_h264::H264Dec::new())
            })
        };

        let mut v_enc: Option<VEnc> = match args.vcodec {
            VideoCodec::Copy => None,
            VideoCodec::H264 => Some(VEnc::H264(
                enc_h264::H264Enc::new(dst_w, dst_h, args.crf, info.fps_num, info.fps_den)
                    .context("H.264 encoder")?
            )),
            VideoCodec::H265 => Some(VEnc::H265(
                enc_h265::H265Enc::new(dst_w, dst_h, info.fps_num, info.fps_den,
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

        // Collected encoded packets: (pts_ms, data, keyframe)
        let mut v_pkts: Vec<(i64, Vec<u8>, bool)> = Vec::new();
        let mut a_pkts: Vec<(i64, Vec<u8>)>       = Vec::new();

        // Read frames via webm-iterable
        let src_file = std::fs::File::open(&args.input)
            .with_context(|| format!("opening {}", args.input.display()))?;
        let tag_iter = WebmIterator::new(src_file, &[]);
        let mut cluster_ts: i64 = 0;

        for tag in tag_iter {
            let tag = tag.context("matroska read tag")?;
            match &tag {
                MatroskaSpec::Cluster(Master::Start) => {}
                MatroskaSpec::Timestamp(ts) => { cluster_ts = *ts as i64; }
                MatroskaSpec::SimpleBlock(raw) => {
                    let sb: SimpleBlock = raw.as_slice().try_into()
                        .context("SimpleBlock parse")?;
                    let abs_pts = cluster_ts + sb.timestamp as i64;
                    let frames = sb.read_frame_data().context("read frame data")?;
                    let data: Vec<u8> = frames.iter().flat_map(|f| f.data.iter().copied()).collect();

                    if sb.track == info.v_track {
                        if v_dec.is_none() {
                            v_pkts.push((abs_pts, data, sb.keyframe));
                        } else {
                            let yuv_frames = match v_dec.as_mut().unwrap() {
                                VDec::H264(d) => d.decode(&data, is_avcc)?,
                                VDec::H265(d) => d.decode(&data, abs_pts)?,
                                VDec::Vp9(d)  => d.decode(&data, abs_pts)?,
                                VDec::Av1(d)  => d.decode(&data, abs_pts)?,
                            };
                            for yuv in yuv_frames {
                                let yuv = if dst_w != yuv.w || dst_h != yuv.h {
                                    yuv.resize(dst_w, dst_h)
                                } else { yuv };
                                let enc_data = encode_frame(&mut v_enc, &yuv)?;
                                if !enc_data.is_empty() {
                                    v_pkts.push((yuv.pts, enc_data, sb.keyframe));
                                }
                            }
                        }
                    } else if Some(sb.track) == info.a_track {
                        a_pkts.push((abs_pts, data));
                    }
                }
                _ => {}
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
                let enc_data = encode_frame(&mut v_enc, &yuv)?;
                if !enc_data.is_empty() { v_pkts.push((yuv.pts, enc_data, false)); }
            }
        }

        // Flush encoders
        if let Some(ref mut enc) = v_enc {
            let flush_pkts: Vec<Vec<u8>> = match enc {
                VEnc::H264(e) => e.flush(),
                VEnc::H265(e) => e.flush()?,
                VEnc::Vp9(e)  => e.flush()?,
                VEnc::Av1(e)  => e.flush()?,
            };
            let last_pts = v_pkts.last().map(|p| p.0).unwrap_or(0);
            for (i, pkt) in flush_pkts.into_iter().enumerate() {
                if !pkt.is_empty() { v_pkts.push((last_pts + i as i64 + 1, pkt, false)); }
            }
        }

        let v_codec_id = match args.vcodec {
            VideoCodec::Copy => info.codec_id.as_str(),
            VideoCodec::H264 => "V_MPEG4/ISO/AVC",
            VideoCodec::H265 => "V_MPEGH/ISO/HEVC",
            VideoCodec::Vp9  => "V_VP9",
            VideoCodec::Av1  => "V_AV1",
        };
        let a_codec_id = match args.acodec {
            VideoAudioCodec::Copy => info.a_codec_id.as_str(),
            VideoAudioCodec::Opus => "A_OPUS",
            VideoAudioCodec::Aac  => "A_AAC",
            VideoAudioCodec::Flac => "A_FLAC",
        };

        write_mkv(&args, &v_pkts, &a_pkts, sub_bytes.as_deref(),
            info.v_track, info.a_track,
            dst_w, dst_h, v_codec_id, a_codec_id)?;

        println!("{} → {}  ({}×{}  {:?}  CRF {})",
            args.input.display(), args.output.display(), dst_w, dst_h, args.vcodec, args.crf);
        Ok(())
    }

    fn encode_frame(enc: &mut Option<VEnc>, yuv: &YuvFrame) -> Result<Vec<u8>> {
        match enc.as_mut() {
            None                => Ok(Vec::new()),
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
        v_track_num: u64,
        a_track_num: Option<u64>,
        dst_w: u32, dst_h: u32,
        v_codec_id:  &str,
        a_codec_id:  &str,
    ) -> Result<()> {
        let dest = std::fs::File::create(&args.output)
            .with_context(|| format!("creating {}", args.output.display()))?;
        let mut writer = WebmWriter::new(dest);

        // EBML header
        writer.write(&MatroskaSpec::Ebml(Master::Start)).context("write EBML")?;
        writer.write(&MatroskaSpec::EbmlVersion(1)).ok();
        writer.write(&MatroskaSpec::EbmlReadVersion(1)).ok();
        writer.write(&MatroskaSpec::EbmlMaxIdLength(4)).ok();
        writer.write(&MatroskaSpec::EbmlMaxSizeLength(8)).ok();
        writer.write(&MatroskaSpec::DocType("matroska".to_owned())).ok();
        writer.write(&MatroskaSpec::DocTypeVersion(4)).ok();
        writer.write(&MatroskaSpec::DocTypeReadVersion(2)).ok();
        writer.write(&MatroskaSpec::Ebml(Master::End)).ok();

        // Segment
        writer.write(&MatroskaSpec::Segment(Master::Start)).context("write Segment")?;

        // Info
        writer.write(&MatroskaSpec::Info(Master::Start)).ok();
        writer.write(&MatroskaSpec::TimestampScale(1_000_000)).ok(); // 1ms units
        writer.write(&MatroskaSpec::MuxingApp("onas".to_owned())).ok();
        writer.write(&MatroskaSpec::WritingApp("onas".to_owned())).ok();
        writer.write(&MatroskaSpec::Info(Master::End)).ok();

        // Tracks
        writer.write(&MatroskaSpec::Tracks(Master::Start)).ok();

        // Video track
        writer.write(&MatroskaSpec::TrackEntry(Master::Start)).ok();
        writer.write(&MatroskaSpec::TrackNumber(v_track_num)).ok();
        writer.write(&MatroskaSpec::TrackUid(v_track_num)).ok();
        writer.write(&MatroskaSpec::TrackType(1)).ok(); // 1=video
        writer.write(&MatroskaSpec::CodecId(v_codec_id.to_owned())).ok();
        writer.write(&MatroskaSpec::Video(Master::Start)).ok();
        writer.write(&MatroskaSpec::PixelWidth(dst_w as u64)).ok();
        writer.write(&MatroskaSpec::PixelHeight(dst_h as u64)).ok();
        writer.write(&MatroskaSpec::Video(Master::End)).ok();
        writer.write(&MatroskaSpec::TrackEntry(Master::End)).ok();

        // Audio track
        if let Some(atn) = a_track_num {
            if !a_pkts.is_empty() {
                writer.write(&MatroskaSpec::TrackEntry(Master::Start)).ok();
                writer.write(&MatroskaSpec::TrackNumber(atn)).ok();
                writer.write(&MatroskaSpec::TrackUid(atn)).ok();
                writer.write(&MatroskaSpec::TrackType(2)).ok(); // 2=audio
                writer.write(&MatroskaSpec::CodecId(a_codec_id.to_owned())).ok();
                writer.write(&MatroskaSpec::TrackEntry(Master::End)).ok();
            }
        }

        // Subtitle track
        let sub_track_num = 3u64;
        if sub_ass.is_some() {
            writer.write(&MatroskaSpec::TrackEntry(Master::Start)).ok();
            writer.write(&MatroskaSpec::TrackNumber(sub_track_num)).ok();
            writer.write(&MatroskaSpec::TrackUid(sub_track_num)).ok();
            writer.write(&MatroskaSpec::TrackType(17)).ok(); // 17=subtitle
            writer.write(&MatroskaSpec::CodecId("S_TEXT/ASS".to_owned())).ok();
            writer.write(&MatroskaSpec::TrackEntry(Master::End)).ok();
        }

        writer.write(&MatroskaSpec::Tracks(Master::End)).ok();

        // Cluster — one cluster, all frames
        writer.write(&MatroskaSpec::Cluster(Master::Start)).context("write Cluster")?;
        writer.write(&MatroskaSpec::Timestamp(0u64)).ok();

        for (pts, data, keyframe) in v_pkts {
            write_simple_block(&mut writer, v_track_num, *pts, *keyframe, data)?;
        }
        if let Some(atn) = a_track_num {
            for (pts, data) in a_pkts {
                write_simple_block(&mut writer, atn, *pts, false, data)?;
            }
        }
        if let Some(ass) = sub_ass {
            write_simple_block(&mut writer, sub_track_num, 0, false, ass)?;
        }

        writer.write(&MatroskaSpec::Cluster(Master::End)).ok();
        writer.write(&MatroskaSpec::Segment(Master::End)).context("write Segment end")?;

        Ok(())
    }

    fn write_simple_block(
        writer:   &mut WebmWriter<std::fs::File>,
        track:    u64,
        pts:      i64,
        keyframe: bool,
        data:     &[u8],
    ) -> Result<()> {
        // SimpleBlock binary: vint(track) | i16be(relative_ts) | flags(u8) | frame_data
        let ts = pts.min(i16::MAX as i64).max(i16::MIN as i64) as i16;
        let mut flags: u8 = 0x00;
        if keyframe { flags |= 0x80; }
        // 1-byte vint for tracks 1-126: 0x80 | n
        let track_vint: Vec<u8> = if track < 0x80 {
            vec![(0x80 | track) as u8]
        } else {
            vec![0x40 | ((track >> 8) & 0x3f) as u8, (track & 0xff) as u8]
        };
        let mut raw = Vec::with_capacity(track_vint.len() + 3 + data.len());
        raw.extend_from_slice(&track_vint);
        raw.extend_from_slice(&ts.to_be_bytes());
        raw.push(flags);
        raw.extend_from_slice(data);
        writer.write(&MatroskaSpec::SimpleBlock(raw)).context("write SimpleBlock")
    }
}
