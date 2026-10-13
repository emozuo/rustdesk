//! 专业模式门面：RustDesk 只依赖这一个 crate。
//!
//! 这里只放与 RustDesk 无关的纯逻辑（协议编解码、版本比对、格式选择）；
//! 和 RustDesk 内部类型打交道的接线在主 crate 的 `src/ddpro/`。

mod protos {
    include!(concat!(env!("OUT_DIR"), "/protos/mod.rs"));
}

pub use protos::ddpro::*;

use protobuf::Message as _;

/// 专业协议版本。两端必须完全一致，不一致就拒绝。
pub const PROTOCOL_VERSION: u32 = 1;

/// 被控端名片在 RustDesk「平台附加信息」里的键，值是 [`PROTOCOL_VERSION`]。
pub const NAME_CARD_KEY: &str = "ddpro";

/// 「专业」档只接受 4:4:4。按偏好从高到低，被控端挑第一个双方都支持的。
const PRO_PREFERENCE: [(Chroma, u32); 2] = [(Chroma::CHROMA_444, 10), (Chroma::CHROMA_444, 8)];

pub fn encode(union: dd_pro::Union) -> protobuf::Result<Vec<u8>> {
    DdPro {
        version: PROTOCOL_VERSION,
        union: Some(union),
        ..Default::default()
    }
    .write_to_bytes()
}

pub fn decode(data: &[u8]) -> protobuf::Result<DdPro> {
    DdPro::parse_from_bytes(data)
}

/// 对端版本和本端不同时，返回写明双方版本、哪一端旧的说明。
pub fn version_mismatch(peer: u32) -> Option<String> {
    if peer == PROTOCOL_VERSION {
        return None;
    }
    let older = if peer < PROTOCOL_VERSION {
        "the peer"
    } else {
        "this side"
    };
    Some(format!(
        "pro protocol version mismatch: this side v{PROTOCOL_VERSION}, peer v{peer}; {older} is older and must be upgraded"
    ))
}

pub fn hello(decode: FormatCaps, display: DisplayCaps) -> dd_pro::Union {
    dd_pro::Union::Hello(Hello {
        tier: Tier::TIER_PRO.into(),
        decode: Some(decode).into(),
        display: Some(display).into(),
        ..Default::default()
    })
}

pub fn reject(reason: reject::Reason, detail: String) -> dd_pro::Union {
    dd_pro::Union::Reject(Reject {
        reason: reason.into(),
        detail,
        ..Default::default()
    })
}

/// 被控端对一次请求的答复，以及哪些能力还没探测（写日志用）。
pub struct Answer {
    pub reply: dd_pro::Union,
    pub unchecked: Vec<&'static str>,
}

/// 被控端拍板：按「专业」档的要求，在本机能编的和对端能解的格式里挑一个。
/// 任何一方的能力还没探测时，只确认协议谈通，不选格式。
pub fn answer_hello(hello: &Hello, encode: &FormatCaps) -> Answer {
    let answer = |reply| Answer {
        reply,
        unchecked: Vec::new(),
    };
    if hello.tier.enum_value() != Ok(Tier::TIER_PRO) {
        return answer(reject(
            reject::Reason::UNSUPPORTED_TIER,
            format!("unsupported tier {:?}", hello.tier),
        ));
    }
    if hello.display.checked && !hello.display.available {
        return answer(reject(
            reject::Reason::DISPLAY_UNAVAILABLE,
            "the controller has no pro display".to_owned(),
        ));
    }

    let unchecked: Vec<&'static str> = [
        (!encode.checked).then_some("encoder"),
        (!hello.decode.checked).then_some("decoder"),
        (!hello.display.checked).then_some("display"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !unchecked.is_empty() {
        return Answer {
            reply: dd_pro::Union::Accept(Accept::default()),
            unchecked,
        };
    }

    let supports = |caps: &FormatCaps, (chroma, depth): (Chroma, u32)| {
        caps.formats
            .iter()
            .any(|f| f.chroma.enum_value() == Ok(chroma) && f.bit_depth == depth)
    };
    match PRO_PREFERENCE
        .into_iter()
        .find(|&want| supports(encode, want) && supports(&hello.decode, want))
    {
        Some((chroma, bit_depth)) => answer(dd_pro::Union::Accept(Accept {
            format: Some(VideoFormat {
                chroma: chroma.into(),
                bit_depth,
                ..Default::default()
            })
            .into(),
            ..Default::default()
        })),
        None => answer(reject(
            reject::Reason::NO_COMMON_FORMAT,
            format!(
                "no 4:4:4 format both sides support; encoder: {}, decoder: {}",
                describe_caps(encode),
                describe_caps(&hello.decode)
            ),
        )),
    }
}

pub fn describe_format(f: &VideoFormat) -> String {
    let chroma = match f.chroma.enum_value() {
        Ok(Chroma::CHROMA_444) => "4:4:4",
        Ok(Chroma::CHROMA_420) => "4:2:0",
        _ => "unknown chroma",
    };
    format!("{chroma} {}bit", f.bit_depth)
}

pub fn describe_caps(caps: &FormatCaps) -> String {
    if !caps.checked {
        return "unchecked".to_owned();
    }
    if caps.formats.is_empty() {
        return "none".to_owned();
    }
    caps.formats
        .iter()
        .map(describe_format)
        .collect::<Vec<_>>()
        .join(", ")
}
