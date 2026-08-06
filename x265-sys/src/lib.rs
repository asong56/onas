#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]

use std::ffi::{c_char, c_double, c_int, c_void};

#[repr(C)]
pub struct x265_encoder {
    _private: [u8; 0],
}

#[repr(C)]
pub struct x265_picyuv {
    _private: [u8; 0],
}

#[repr(C)]
pub struct x265_nal {
    pub type_:     u32,
    pub sizeBytes: u32,
    pub payload:   *mut u8,
}

#[repr(C)]
pub struct x265_picture {
    pub pts:        i64,
    pub dts:        i64,
    pub userData:   *mut c_void,
    pub planes:     [*mut c_void; 3],
    pub stride:     [c_int; 3],
    pub bitDepth:   c_int,
    pub sliceType:  c_int,
    pub poc:        c_int,
    pub colorSpace: c_int,
    _pad: [u8; 4096],
}

pub const X265_CSP_I420: c_int = 1;
pub const X265_CSP_I422: c_int = 2;
pub const X265_CSP_I444: c_int = 3;

pub const X265_RC_ABR: c_int = 0;
pub const X265_RC_CQP: c_int = 1;
pub const X265_RC_CRF: c_int = 2;

pub const X265_TYPE_AUTO: c_int = 0x0000;
pub const X265_TYPE_IDR:  c_int = 0x0001;
pub const X265_TYPE_I:    c_int = 0x0002;
pub const X265_TYPE_P:    c_int = 0x0003;
pub const X265_TYPE_B:    c_int = 0x0005;

#[repr(C)]
pub struct x265_param {
    _private: [u8; 0],
}

#[link(name = "x265")]
extern "C" {
    pub fn x265_param_alloc() -> *mut x265_param;
    pub fn x265_param_free(param: *mut x265_param);
    pub fn x265_param_default(param: *mut x265_param);

    pub fn x265_param_parse(
        param: *mut x265_param,
        name:  *const c_char,
        value: *const c_char,
    ) -> c_int;

    pub fn x265_param_apply_profile(
        param:   *mut x265_param,
        profile: *const c_char,
    ) -> c_int;

    pub fn x265_param_default_preset(
        param:  *mut x265_param,
        preset: *const c_char,
        tune:   *const c_char,
    ) -> c_int;
    
    pub fn x265_picture_alloc() -> *mut x265_picture;
    pub fn x265_picture_free(pic: *mut x265_picture);
    pub fn x265_picture_init(param: *mut x265_param, pic: *mut x265_picture);

    pub fn x265_encoder_headers(
        encoder: *mut x265_encoder,
        pp_nal:  *mut *mut x265_nal,
        pi_nal:  *mut u32,
    ) -> c_int;

    pub fn x265_encoder_encode(
        encoder: *mut x265_encoder,
        pp_nal:  *mut *mut x265_nal,
        pi_nal:  *mut u32,
        pic_in:  *mut x265_picture,
        pic_out: *mut x265_picture,
    ) -> c_int;

    pub fn x265_encoder_get_stats(
        encoder:        *mut x265_encoder,
        stats:          *mut x265_stats,
        statsSizeBytes: u32,
    );

    pub fn x265_encoder_close(encoder: *mut x265_encoder);

    pub fn x265_cleanup();
    pub static x265_version_str:    *const c_char;
    pub static x265_build_info_str: *const c_char;
    pub static x265_max_bit_depth:  c_int;
}

#[repr(C)]
pub struct x265_stats {
    pub globalPsnrY:           c_double,
    pub globalPsnrU:           c_double,
    pub globalPsnrV:           c_double,
    pub globalPsnr:            c_double,
    pub globalSsim:            c_double,
    pub elapsedEncodeTime:     c_double,
    pub elapsedVideoTime:      c_double,
    pub bitrate:               c_double,
    pub aggregateVmafScore:    c_double,
    pub accBits:               u64,
    pub encodedPictureCount:   u32,
    pub totalWPFrames:         u32,
    _pad: [u8; 512],
}

impl Default for x265_stats {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

pub const X265_PARAM_BAD_NAME:  c_int = -1;
pub const X265_PARAM_BAD_VALUE: c_int = -2;

#[repr(C)]
pub struct x265_api {
    pub api_major_version: c_int,
    pub api_build_number:  c_int,
    pub sizeof_param:          c_int,
    pub sizeof_picture:        c_int,
    pub sizeof_analysis_data:  c_int,
    pub sizeof_zone:           c_int,
    pub sizeof_stats:          c_int,
    pub bit_depth:      c_int,
    pub version_str:    *const c_char,
    pub build_info_str: *const c_char,
    pub param_alloc:   Option<unsafe extern "C" fn() -> *mut x265_param>,
    pub param_free:    Option<unsafe extern "C" fn(*mut x265_param)>,
    pub param_default: Option<unsafe extern "C" fn(*mut x265_param)>,
    pub param_parse:   Option<unsafe extern "C" fn(*mut x265_param, *const c_char, *const c_char) -> c_int>,
    pub scenecut_aware_qp_param_parse:
                       Option<unsafe extern "C" fn(*mut x265_param, *const c_char, *const c_char) -> c_int>,
    pub param_apply_profile:  Option<unsafe extern "C" fn(*mut x265_param, *const c_char) -> c_int>,
    pub param_default_preset: Option<unsafe extern "C" fn(*mut x265_param, *const c_char, *const c_char) -> c_int>,
    pub picture_alloc: Option<unsafe extern "C" fn() -> *mut x265_picture>,
    pub picture_free:  Option<unsafe extern "C" fn(*mut x265_picture)>,
    pub picture_init:  Option<unsafe extern "C" fn(*mut x265_param, *mut x265_picture)>,
    pub encoder_open:  Option<unsafe extern "C" fn(*mut x265_param) -> *mut x265_encoder>,
}

#[link(name = "x265")]
extern "C" {
    pub fn x265_api_query(bit_depth: c_int, api_version: c_int, err: *mut c_int) -> *const x265_api;
}

extern "C" {
    fn onas_x265_build_number() -> c_int;
}

pub const X265_API_QUERY_ERR_NONE:           c_int = 0;
pub const X265_API_QUERY_ERR_VER_REFUSED:    c_int = 1;
pub const X265_API_QUERY_ERR_LIB_NOT_FOUND:  c_int = 2;
pub const X265_API_QUERY_ERR_FUNC_NOT_FOUND: c_int = 3;
pub const X265_API_QUERY_ERR_WRONG_BITDEPTH: c_int = 4;

pub unsafe fn x265_encoder_open(param: *mut x265_param) -> *mut x265_encoder {
    let build = onas_x265_build_number();
    let mut err: c_int = 0;
    let api = x265_api_query(8, build, &mut err);
    if api.is_null() {
        return std::ptr::null_mut();
    }
    match (*api).encoder_open {
        Some(f) => f(param),
        None    => std::ptr::null_mut(),
    }
}
