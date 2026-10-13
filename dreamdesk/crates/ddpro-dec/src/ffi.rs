#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code, clippy::all)]

// 链接靠 hwcodec（它静态链接 vcpkg 的 FFmpeg）；这里不引用它的任何符号，显式引用一下，确保它被链接进来
use hwcodec as _;

include!(concat!(env!("OUT_DIR"), "/ffi.rs"));
