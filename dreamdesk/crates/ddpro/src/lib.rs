//! 专业模式门面：RustDesk 只依赖这一个 crate。
//!
//! 这里只放与 RustDesk 无关的纯逻辑（协议编解码、版本比对、格式选择）；
//! 和 RustDesk 内部类型打交道的接线在主 crate 的 `src/ddpro/`。

mod protos {
    include!(concat!(env!("OUT_DIR"), "/protos/mod.rs"));
}

pub use protos::ddpro::*;
// RustDesk 只依赖门面这一个 crate，编码器和画面通道经这里拿
pub use ddpro_enc as enc;
pub use ddpro_media as media;

use protobuf::Message as _;

/// 专业协议版本。两端必须完全一致，不一致就拒绝。
pub const PROTOCOL_VERSION: u32 = 4;

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

pub fn hello(decode: FormatCaps, display: DisplayCaps, display_index: u32) -> dd_pro::Union {
    dd_pro::Union::Hello(Hello {
        tier: Tier::TIER_PRO.into(),
        decode: Some(decode).into(),
        display: Some(display).into(),
        display_index,
        ..Default::default()
    })
}

/// 画面通道信息（地址 + 密钥材料），放进「同意」答复。
pub fn media_channel(host: std::net::IpAddr, port: u16, keys: &media::Keys) -> MediaChannel {
    MediaChannel {
        host: host.to_string(),
        port: port as u32,
        token: keys.token.to_vec(),
        srtp_key: keys.key.to_vec(),
        srtp_salt: keys.salt.to_vec(),
        ssrc_media: keys.ssrc_media,
        ssrc_control: keys.ssrc_control,
        ..Default::default()
    }
}

/// 主控端从「同意」答复里取出通道地址和密钥；字段不全或长度不对返回 None。
pub fn media_endpoint(m: &MediaChannel) -> Option<(std::net::SocketAddr, media::Keys)> {
    let ip: std::net::IpAddr = m.host.parse().ok()?;
    let port = u16::try_from(m.port).ok().filter(|&p| p != 0)?;
    let keys = media::Keys {
        key: m.srtp_key.as_slice().try_into().ok()?,
        salt: m.srtp_salt.as_slice().try_into().ok()?,
        token: m.token.as_slice().try_into().ok()?,
        ssrc_media: m.ssrc_media,
        ssrc_control: m.ssrc_control,
    };
    Some(((ip, port).into(), keys))
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

    let supports = |caps: &FormatCaps, (chroma, depth): (Chroma, u32)| {
        caps.formats
            .iter()
            .any(|f| f.chroma.enum_value() == Ok(chroma) && f.bit_depth == depth)
    };
    // 已经探测过、明确做不到的一方先判：不必等另一方探测完才拒绝
    for (side, caps) in [("encoder", encode), ("decoder", &*hello.decode)] {
        if caps.checked && !PRO_PREFERENCE.into_iter().any(|want| supports(caps, want)) {
            return answer(reject(
                reject::Reason::NO_COMMON_FORMAT,
                format!("the {side} cannot do 4:4:4 (has: {})", describe_caps(caps)),
            ));
        }
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

/// 被控端此刻能编的格式。`probe` 是用要编的那块屏的采集器所在显卡探测的结果（每次请求时现查：
/// 编码会话占用是实时状态）。探测不成功也算「已探测、能编的为空」，不当作「未探测」；
/// 第二个值是做不到的原因，给日志用。
pub fn encoder_caps(probe: Result<enc::Caps, enc::Error>) -> (FormatCaps, Option<String>) {
    let mut caps = FormatCaps {
        checked: true,
        ..Default::default()
    };
    match probe {
        // 编码器目前只实现了 8bit；10bit 等 P1 做 HDR 时再报
        Ok(c) if c.yuv444_8bit => {
            caps.formats.push(VideoFormat {
                chroma: Chroma::CHROMA_444.into(),
                bit_depth: 8,
                ..Default::default()
            });
            (caps, None)
        }
        Ok(_) => (caps, Some("this GPU cannot encode HEVC 4:4:4".to_owned())),
        Err(e) => (caps, Some(e.to_string())),
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
