//! 虚拟麦克风（P2 第 2 项），被控端 Windows：专业会话的语音通话不放扬声器，送进虚拟麦克风驱动
//! （`dreamdesk/driver/mic`），会议、录音软件选「DreamDesk Virtual Audio」麦克风就录到主控端的人说话。
//!
//! 传输、编解码都是 RustDesk 语音通话原有的；这里只换掉被控端最后一步（`connection.rs` 的三个钩子）：
//! 专业会话自动接受通话、不把采集源切到麦克风（切过去会把虚拟麦克风里主控端自己的声音采回去）、语音帧交给这里。
//! 驱动没装、不是专业会话的连接，一律走 RustDesk 原路径。

use crate::client::{MediaData, MediaSender};
use hbb_common::{log, throttled_log};
use magnum_opus::{Channels::Mono, Decoder};
use std::{
    collections::HashSet,
    sync::{mpsc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use winapi::{
    shared::winerror::ERROR_NOT_READY,
    um::{
        fileapi::{CreateFileW, OPEN_EXISTING},
        handleapi::{CloseHandle, INVALID_HANDLE_VALUE},
        ioapiset::DeviceIoControl,
        winnt::{FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_WRITE, HANDLE},
    },
};

/// 驱动的控制设备（只有 SYSTEM / 管理员能开，被控端的 `--server` 是 SYSTEM）
const DEVICE: &str = r"\\.\DreamDeskMicCtl";
/// `IOCTL_DD_MIC_WRITE_PCM`（`dreamdesk/driver/mic/include/dd_mic_ioctl.h`）：
/// CTL_CODE(0x8901, 0x802, METHOD_BUFFERED, FILE_WRITE_ACCESS)，输入是单声道 S16 48 kHz PCM
const IOCTL_WRITE_PCM: u32 = (0x8901 << 16) | (2 << 14) | (0x802 << 2);
/// 驱动收的格式。Opus 解码器能把任何采样率 / 声道数的码流直接解成这个格式，不用另做重采样和混音
const SAMPLE_RATE: u32 = 48000;
/// Opus 一包最长 120 ms
const MAX_FRAME_SAMPLES: usize = SAMPLE_RATE as usize * 120 / 1000;
/// 驱动的缓冲只能有一个写入者：正在写的连接停了这么久，别的连接才能接手
const OWNER_IDLE: Duration = Duration::from_millis(500);
const LOG_INTERVAL: Duration = Duration::from_secs(60);

/// 有专业会话的连接（谈成过专业会话就算，之后切回通用模式也算：用户在专业窗口里开的麦克风）
fn pro_conns() -> &'static Mutex<HashSet<i32>> {
    static S: OnceLock<Mutex<HashSet<i32>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// 正在往驱动写的连接和它最后一次写的时刻
fn owner() -> &'static Mutex<Option<(i32, Instant)>> {
    static S: OnceLock<Mutex<Option<(i32, Instant)>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// 专业会话谈成（`server.rs`）
pub fn on_pro_session(conn_id: i32) {
    pro_conns().lock().unwrap().insert(conn_id);
}

/// 连接关闭（`server.rs` 的 `on_conn_closed`）
pub fn on_conn_closed(conn_id: i32) {
    pro_conns().lock().unwrap().remove(&conn_id);
    let mut o = owner().lock().unwrap();
    if o.is_some_and(|(id, _)| id == conn_id) {
        *o = None;
    }
}

/// 这条连接的语音通话由虚拟麦克风接：专业会话的连接，且驱动在。`connection.rs` 据此自动接受、不切采集源
pub fn takes_voice_call(conn_id: i32) -> bool {
    pro_conns().lock().unwrap().contains(&conn_id) && Device::open().is_ok()
}

/// 代替 RustDesk 的播放线程接这条连接的语音帧（`connection.rs` 收到语音格式时）；不归虚拟麦克风管时返回 None
pub fn audio_sender(conn_id: i32) -> Option<MediaSender> {
    if !takes_voice_call(conn_id) {
        return None;
    }
    let (tx, rx) = mpsc::channel::<MediaData>();
    let spawned = std::thread::Builder::new()
        .name("ddpro-mic".into())
        .spawn(move || run(conn_id, rx));
    match spawned {
        Ok(_) => Some(tx),
        Err(e) => {
            log::error!("ddpro: mic: cannot start the voice thread: {e}; playing the voice call on the speakers");
            None
        }
    }
}

fn run(conn_id: i32, rx: mpsc::Receiver<MediaData>) {
    log::info!("ddpro: mic: voice call of connection {conn_id} goes into the virtual microphone");
    let mut decoder = None;
    let mut device = None;
    let mut pcm = vec![0i16; MAX_FRAME_SAMPLES];
    while let Ok(data) = rx.recv() {
        match data {
            MediaData::AudioFormat(f) => {
                // 声音格式变了（主控端重开了采集）：新码流要新的解码器状态
                decoder = match Decoder::new(SAMPLE_RATE, Mono) {
                    Ok(d) => Some(d),
                    Err(e) => {
                        log::error!("ddpro: mic: cannot create the voice decoder for {} Hz: {e:?}", f.sample_rate);
                        None
                    }
                };
            }
            MediaData::AudioFrame(frame) => {
                let Some(d) = decoder.as_mut() else { continue };
                let n = match d.decode(&frame.data, &mut pcm, false) {
                    Ok(n) => n,
                    Err(e) => {
                        throttled_log!(LOG_INTERVAL, warn, "ddpro: mic: cannot decode a voice frame: {e:?}");
                        continue;
                    }
                };
                if n == 0 || !take_turn(conn_id) {
                    continue;
                }
                if device.is_none() {
                    device = match Device::open() {
                        Ok(dev) => Some(dev),
                        Err(e) => {
                            throttled_log!(LOG_INTERVAL, warn, "ddpro: mic: cannot open {DEVICE}: {e}");
                            continue;
                        }
                    };
                }
                if let Some(dev) = &device {
                    match dev.write(&pcm[..n]) {
                        Ok(()) => {}
                        // 没有软件在录：驱动不收，正常
                        Err(e) if e.raw_os_error() == Some(ERROR_NOT_READY as i32) => {}
                        Err(e) => {
                            throttled_log!(LOG_INTERVAL, warn, "ddpro: mic: writing to the virtual microphone failed: {e}");
                            // 驱动被重装 / 设备被移除时旧句柄一直失败：下次重开
                            device = None;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    log::info!("ddpro: mic: voice of connection {conn_id} stopped");
}

/// 轮到这条连接写：没人在写、它自己在写、或正在写的那个停了超过 [`OWNER_IDLE`]
fn take_turn(conn_id: i32) -> bool {
    let mut o = owner().lock().unwrap();
    let now = Instant::now();
    match *o {
        Some((id, last)) if id != conn_id && now.duration_since(last) < OWNER_IDLE => false,
        _ => {
            if o.is_none_or(|(id, _)| id != conn_id) {
                log::info!("ddpro: mic: connection {conn_id} now feeds the virtual microphone");
            }
            *o = Some((conn_id, now));
            true
        }
    }
}

struct Device(HANDLE);

// 句柄只在语音线程里用
unsafe impl Send for Device {}

impl Device {
    fn open() -> std::io::Result<Self> {
        let path: Vec<u16> = DEVICE.encode_utf16().chain(Some(0)).collect();
        let h = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(h))
    }

    fn write(&self, pcm: &[i16]) -> std::io::Result<()> {
        let mut returned = 0;
        let ok = unsafe {
            DeviceIoControl(
                self.0,
                IOCTL_WRITE_PCM,
                pcm.as_ptr() as *mut _,
                std::mem::size_of_val(pcm) as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
