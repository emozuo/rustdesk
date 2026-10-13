//! Linux 上按 vcpkg 里那份 FFmpeg 的头文件生成接口绑定；其它平台这个 crate 是空的。

fn main() {
    println!("cargo:rerun-if-changed=src/ffi.h");
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
    #[cfg(target_os = "linux")]
    linux::bindings();
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{env, path::PathBuf};

    /// vcpkg 的安装目录和 hwcodec 找的是同一个（`$VCPKG_ROOT/installed/<架构>-linux`）。
    fn vcpkg_include() -> PathBuf {
        let root = env::var("VCPKG_ROOT").expect("VCPKG_ROOT must point at the vcpkg used for RustDesk's FFmpeg");
        let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default().as_str() {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            other => panic!("unsupported architecture for the pro decoder: {other}"),
        };
        PathBuf::from(root).join("installed").join(format!("{arch}-linux")).join("include")
    }

    pub fn bindings() {
        let include = vcpkg_include();
        bindgen::Builder::default()
            .header("src/ffi.h")
            .clang_arg(format!("-I{}", include.display()))
            .allowlist_function(
                "avcodec_(find_decoder|alloc_context3|free_context|open2|send_packet|receive_frame)\
                 |av_packet_(alloc|free|unref)|av_new_packet|av_frame_(alloc|free)\
                 |av_buffer_(ref|unref)|av_hwdevice_ctx_create|av_get_pix_fmt_name|av_pix_fmt_desc_get|av_strerror",
            )
            .allowlist_type(
                "AVVAAPIDeviceContext|AVHWDeviceContext|AVHWFramesContext|VAAPIDynLoadFunctions\
                 |VAProcPipelineParameterBuffer|VADRMPRIMESurfaceDescriptor|VAImage|VAImageFormat\
                 |VASurfaceAttrib|VARectangle",
            )
            .allowlist_var("VA_.*|AV_CODEC_FLAG_LOW_DELAY")
            .default_enum_style(bindgen::EnumVariation::Consts)
            .prepend_enum_name(false)
            .layout_tests(false)
            .generate()
            .expect("generate FFmpeg / VAAPI bindings")
            .write_to_file(PathBuf::from(env::var("OUT_DIR").unwrap()).join("ffi.rs"))
            .expect("write FFmpeg / VAAPI bindings");
    }
}
