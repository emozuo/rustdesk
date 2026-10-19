//! H.265 码流在 RTP 里的切包与组装（RFC 7798，只用「单个 NAL」和「分片 FU」两种包）。
//!
//! 没用 rtp 库自带的 HevcPayloader：它遇到不认识的 NAL 类型会悄悄丢掉，3 / 4 字节起始码混用时也会
//! 切错——和「不静默降级」「传输无损」冲突。这里任何 NAL 都原样送达，收到看不懂的包就报错。

pub(crate) const NAL_HEADER: usize = 2;
const TYPE_AP: u8 = 48;
const TYPE_FU: u8 = 49;
const START_CODE: [u8; 4] = [0, 0, 0, 1];

pub fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |b| (b >> 1) & 0x3f)
}

/// 关键帧（IRAP：BLA / IDR / CRA），从它开始可以独立解码。
pub fn is_irap(nal: &[u8]) -> bool {
    is_irap_type(nal_type(nal))
}

fn is_irap_type(t: u8) -> bool {
    (16..=21).contains(&t)
}

/// 这个 RTP 负载是不是关键帧开头的部分：参数集（VPS / SPS / PPS）或 IRAP 的单个 NAL / 首个分片。
pub fn payload_starts_keyframe(p: &[u8]) -> bool {
    let t = match nal_type(p) {
        TYPE_FU if p.len() > NAL_HEADER && p[2] & 0x80 != 0 => p[2] & 0x3f,
        t => t,
    };
    is_irap_type(t) || (32..=34).contains(&t)
}

/// 按起始码（00 00 01 或 00 00 00 01）切出各个 NAL，不含起始码。
pub fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(data.len(), |&n| n - 3);
        // 4 字节起始码多出来的那个 0 属于下一个起始码，不属于本 NAL
        while k + 1 < starts.len() && end > s && data[end - 1] == 0 {
            end -= 1;
        }
        if end > s {
            nals.push(&data[s..end]);
        }
    }
    nals
}

/// 一个 NAL 切成 RTP 负载：放得下就整包，放不下就切成 FU 分片。
pub fn payload(nal: &[u8], max: usize, out: &mut Vec<Vec<u8>>) {
    if nal.len() <= max {
        out.push(nal.to_vec());
        return;
    }
    let (h0, h1) = (nal[0], nal[1]);
    let fu_type = nal_type(nal);
    // 分片包头：类型改成 49，F / LayerId / TID 照抄原 NAL 头
    let hdr = [(h0 & 0x81) | (TYPE_FU << 1), h1];
    let chunk = max - NAL_HEADER - 1;
    let body = &nal[NAL_HEADER..];
    let n = body.len().div_ceil(chunk);
    for (k, part) in body.chunks(chunk).enumerate() {
        let mut fu = 0u8;
        if k == 0 {
            fu |= 0x80;
        }
        if k + 1 == n {
            fu |= 0x40;
        }
        let mut p = Vec::with_capacity(3 + part.len());
        p.extend_from_slice(&hdr);
        p.push(fu | fu_type);
        p.extend_from_slice(part);
        out.push(p);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DepacketizeError {
    /// 本端只发单个 NAL 和 FU，收到聚合包、PACI 等说明对端不是我们自己的发送端
    Unsupported(u8),
    /// 分片没有起始片就来了中间片 / 结束片（丢包时会出现，调用方按丢包处理）
    BrokenFragment,
    TooShort,
}

/// 把一帧的 RTP 负载拼回带起始码的码流。
#[derive(Default)]
pub struct Depacketizer {
    frame: Vec<u8>,
    fu: Option<Vec<u8>>,
}

impl Depacketizer {
    pub fn push(&mut self, p: &[u8]) -> Result<(), DepacketizeError> {
        if p.len() < NAL_HEADER {
            return Err(DepacketizeError::TooShort);
        }
        match nal_type(p) {
            TYPE_FU => {
                if p.len() < NAL_HEADER + 1 {
                    return Err(DepacketizeError::TooShort);
                }
                let fu = p[2];
                let data = &p[3..];
                if fu & 0x80 != 0 {
                    // 上一个分片 NAL 还没收到结尾就来了新的开头：中间丢了包，不能悄悄扔掉半个 NAL 接着拼
                    if self.fu.take().is_some() {
                        return Err(DepacketizeError::BrokenFragment);
                    }
                    let mut nal = vec![(p[0] & 0x81) | ((fu & 0x3f) << 1), p[1]];
                    nal.extend_from_slice(data);
                    self.fu = Some(nal);
                } else {
                    self.fu.as_mut().ok_or(DepacketizeError::BrokenFragment)?.extend_from_slice(data);
                }
                if fu & 0x40 != 0 {
                    let nal = self.fu.take().ok_or(DepacketizeError::BrokenFragment)?;
                    self.frame.extend_from_slice(&START_CODE);
                    self.frame.extend_from_slice(&nal);
                }
                Ok(())
            }
            TYPE_AP | 50..=63 => Err(DepacketizeError::Unsupported(nal_type(p))),
            _ => {
                self.frame.extend_from_slice(&START_CODE);
                self.frame.extend_from_slice(p);
                Ok(())
            }
        }
    }

    /// 一帧收齐（RTP marker）时取走。还有没拼完的分片说明这一帧缺包。
    pub fn take_frame(&mut self) -> Result<Vec<u8>, DepacketizeError> {
        if self.fu.take().is_some() {
            self.frame.clear();
            return Err(DepacketizeError::BrokenFragment);
        }
        Ok(std::mem::take(&mut self.frame))
    }

    pub fn reset(&mut self) {
        self.frame.clear();
        self.fu = None;
    }
}

/// 把码流切成一帧一帧（access unit）：一帧从参数集 / SEI 或首个分片（first_slice_segment_in_pic_flag）开始。
/// 只给验证工具回放录好的码流用；编码器每次本来就只交出一帧。
pub fn split_access_units(data: &[u8]) -> Vec<Vec<u8>> {
    let mut aus: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut cur_has_vcl = false;
    for nal in split_annexb(data) {
        let t = nal_type(nal);
        let vcl = t < 32;
        let first_slice = vcl && nal.len() > NAL_HEADER && nal[NAL_HEADER] & 0x80 != 0;
        // 新一帧的开始：已经有画面数据之后，又遇到非画面 NAL 或新一帧的首个分片
        if cur_has_vcl && (!vcl || first_slice) {
            aus.push(std::mem::take(&mut cur));
            cur_has_vcl = false;
        }
        cur.extend_from_slice(&START_CODE);
        cur.extend_from_slice(nal);
        cur_has_vcl |= vcl;
    }
    if !cur.is_empty() {
        aus.push(cur);
    }
    aus
}
