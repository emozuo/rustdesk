//! HDR 输出（P1 第 4 项）：Wayland 颜色管理协议（wp_color_manager_v1，GNOME 48 起支持）。
//!
//! - [`probe_hdr`]：会话进程问本机能不能显示 HDR——合成器支持「按参数描述的画面」、PQ 编码和 BT.2020 原色，而且每块屏此刻
//!   都开着 HDR。能才告诉被控端「我能收 HDR」（10bit）；
//! - 专业窗口（`wayland.rs`）把画面表面标成 BT.2020 + PQ，画面原样交给合成器，由它送到 HDR 屏（或按屏的能力映射）。
//!
//! 事件处理（[`ColorData`]）两边共用：探测和专业窗口各自的事件队列状态把这些对象的事件转给它。

use sctk::reexports::{
    client::{
        backend::ObjectId,
        globals::{registry_queue_init, GlobalListContents},
        protocol::{wl_output::WlOutput, wl_registry},
        Connection, Dispatch, Proxy, QueueHandle, WEnum,
    },
    protocols::wp::color_management::v1::client::{
        wp_color_management_output_v1::{self, WpColorManagementOutputV1},
        wp_color_management_surface_v1::{self, WpColorManagementSurfaceV1},
        wp_color_manager_v1::{self, Feature, Primaries, RenderIntent, TransferFunction, WpColorManagerV1},
        wp_image_description_creator_params_v1::{self, WpImageDescriptionCreatorParamsV1},
        wp_image_description_info_v1::{self, WpImageDescriptionInfoV1},
        wp_image_description_v1::{self, WpImageDescriptionV1},
    },
};
use std::collections::HashMap;

/// 合成器报的颜色管理能力，以及建出来的图像描述、查到的描述信息
#[derive(Default)]
pub(crate) struct ColorData {
    features: Vec<Feature>,
    tfs: Vec<TransferFunction>,
    primaries: Vec<Primaries>,
    intents: Vec<RenderIntent>,
    /// 合成器报完了能力（`done` 事件）
    done: bool,
    /// 图像描述 → 建好了（Ok）/ 建不了（原因）
    descriptions: HashMap<ObjectId, Result<(), String>>,
    /// 描述信息对象 → 收到的信息
    infos: HashMap<ObjectId, Info>,
}

/// 一个图像描述的信息（只取判断「是不是 HDR」用得到的）
#[derive(Default, Clone, Debug)]
pub(crate) struct Info {
    tf: Option<TransferFunction>,
    /// (最暗 × 10000, 最亮, 参考白)，单位尼特
    luminances: Option<(u32, u32, u32)>,
    /// 目标显示的最亮（尼特）
    target_max: Option<u32>,
    done: bool,
}

impl Info {
    /// 屏此刻在 HDR 模式：按 PQ / HLG 编码，或目标亮度比参考白（普通内容的白）亮
    fn is_hdr(&self) -> bool {
        matches!(self.tf, Some(TransferFunction::St2084Pq | TransferFunction::Hlg))
            || matches!((self.target_max, self.luminances), (Some(max), Some((_, _, white))) if max > white)
    }
}

impl ColorData {
    /// 合成器能不能接 BT.2020 + PQ 的画面
    pub(crate) fn hdr_capable(&self) -> Result<(), String> {
        if !self.done {
            return Err("the compositor did not report its color management features".into());
        }
        if !self.features.contains(&Feature::Parametric) {
            return Err("the compositor takes no parametric image descriptions".into());
        }
        if !self.tfs.contains(&TransferFunction::St2084Pq) {
            return Err("the compositor does not support the PQ transfer function".into());
        }
        if !self.primaries.contains(&Primaries::Bt2020) {
            return Err("the compositor does not support BT.2020 primaries".into());
        }
        if !self.intents.contains(&RenderIntent::Perceptual) {
            return Err("the compositor does not support the perceptual rendering intent".into());
        }
        Ok(())
    }

    /// 合成器报完能力了没有
    pub(crate) fn reported(&self) -> bool {
        self.done
    }

    pub(crate) fn sets_luminances(&self) -> bool {
        self.features.contains(&Feature::SetLuminances)
    }

    /// 图像描述建好了没有：None = 还没回音
    pub(crate) fn description(&self, d: &WpImageDescriptionV1) -> Option<Result<(), String>> {
        self.descriptions.get(&d.id()).cloned()
    }
}

/// 给事件队列状态用：把颜色管理对象的事件转给它带着的 [`ColorData`]
pub(crate) trait HasColor {
    fn color(&mut self) -> &mut ColorData;
}

impl<D: HasColor + Dispatch<WpColorManagerV1, ()>> Dispatch<WpColorManagerV1, (), D> for ColorData {
    fn event(state: &mut D, _: &WpColorManagerV1, event: wp_color_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<D>) {
        let c = state.color();
        match event {
            wp_color_manager_v1::Event::SupportedIntent { render_intent: WEnum::Value(v) } => c.intents.push(v),
            wp_color_manager_v1::Event::SupportedFeature { feature: WEnum::Value(v) } => c.features.push(v),
            wp_color_manager_v1::Event::SupportedTfNamed { tf: WEnum::Value(v) } => c.tfs.push(v),
            wp_color_manager_v1::Event::SupportedPrimariesNamed { primaries: WEnum::Value(v) } => c.primaries.push(v),
            wp_color_manager_v1::Event::Done => c.done = true,
            _ => {}
        }
    }
}

impl<D: HasColor + Dispatch<WpImageDescriptionV1, ()>> Dispatch<WpImageDescriptionV1, (), D> for ColorData {
    fn event(state: &mut D, d: &WpImageDescriptionV1, event: wp_image_description_v1::Event, _: &(), _: &Connection, _: &QueueHandle<D>) {
        let result = match event {
            wp_image_description_v1::Event::Ready { .. } => Ok(()),
            wp_image_description_v1::Event::Failed { cause, msg } => Err(format!("{cause:?}: {msg}")),
            _ => return,
        };
        state.color().descriptions.insert(d.id(), result);
    }
}

impl<D: HasColor + Dispatch<WpImageDescriptionInfoV1, ()>> Dispatch<WpImageDescriptionInfoV1, (), D> for ColorData {
    fn event(
        state: &mut D,
        info: &WpImageDescriptionInfoV1,
        event: wp_image_description_info_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
        let i = state.color().infos.entry(info.id()).or_default();
        match event {
            wp_image_description_info_v1::Event::TfNamed { tf: WEnum::Value(v) } => i.tf = Some(v),
            wp_image_description_info_v1::Event::Luminances {
                min_lum,
                max_lum,
                reference_lum,
            } => i.luminances = Some((min_lum, max_lum, reference_lum)),
            wp_image_description_info_v1::Event::TargetLuminance { max_lum, .. } => i.target_max = Some(max_lum),
            wp_image_description_info_v1::Event::Done => i.done = true,
            _ => {}
        }
    }
}

impl<D: HasColor + Dispatch<WpImageDescriptionCreatorParamsV1, ()>> Dispatch<WpImageDescriptionCreatorParamsV1, (), D> for ColorData {
    fn event(
        _: &mut D,
        _: &WpImageDescriptionCreatorParamsV1,
        _: wp_image_description_creator_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

impl<D: HasColor + Dispatch<WpColorManagementSurfaceV1, ()>> Dispatch<WpColorManagementSurfaceV1, (), D> for ColorData {
    fn event(
        _: &mut D,
        _: &WpColorManagementSurfaceV1,
        _: wp_color_management_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

impl<D: HasColor + Dispatch<WpColorManagementOutputV1, ()>> Dispatch<WpColorManagementOutputV1, (), D> for ColorData {
    fn event(
        _: &mut D,
        _: &WpColorManagementOutputV1,
        _: wp_color_management_output_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

/// 本机能不能显示 HDR：合成器能接 BT.2020 + PQ 的画面，而且每块屏此刻都开着 HDR（只开了一块时不报：
/// 专业窗口可能落在没开的那块上，合成器把 HDR 压回普通画面不如被控端直接给 8bit 准）。不能时返回原因。
/// 每次现查（一次连接、几次往返，几毫秒）：用户在系统设置里开关 HDR 后，下次请求专业模式就按新状态谈。
pub fn probe_hdr() -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("cannot reach the Wayland display: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<HdrProbe>(&conn).map_err(|e| format!("read the Wayland globals: {e}"))?;
    let qh = queue.handle();
    let cm = globals
        .bind::<WpColorManagerV1, _, _>(&qh, 1..=1, ())
        .map_err(|_| "the desktop has no color management (wp_color_manager_v1, GNOME 48 or later)".to_string())?;
    let outputs: Vec<WlOutput> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == WlOutput::interface().name)
            .map(|g| globals.registry().bind::<WlOutput, _, _>(g.name, g.version.min(4), &qh, ()))
            .collect()
    });
    let mut p = HdrProbe { color: ColorData::default() };
    let roundtrip = |queue: &mut sctk::reexports::client::EventQueue<HdrProbe>, p: &mut HdrProbe| {
        queue.roundtrip(p).map(|_| ()).map_err(|e| format!("talk to the compositor: {e}"))
    };
    roundtrip(&mut queue, &mut p)?;
    p.color.hdr_capable()?;
    if outputs.is_empty() {
        return Err("the compositor reports no screen".into());
    }
    let watched: Vec<WpColorManagementOutputV1> = outputs.iter().map(|o| cm.get_output(o, &qh, ())).collect();
    let descriptions: Vec<WpImageDescriptionV1> = watched.iter().map(|w| w.get_image_description(&qh, ())).collect();
    roundtrip(&mut queue, &mut p)?;
    let mut infos = Vec::new();
    for (i, d) in descriptions.iter().enumerate() {
        match p.color.description(d) {
            Some(Ok(())) => infos.push(d.get_information(&qh, ())),
            Some(Err(e)) => return Err(format!("screen {} has no color information: {e}", i + 1)),
            None => return Err(format!("screen {}: the compositor did not describe its colors", i + 1)),
        }
    }
    roundtrip(&mut queue, &mut p)?;
    for (i, info) in infos.iter().enumerate() {
        let got = p.color.infos.get(&info.id()).cloned().unwrap_or_default();
        if !got.is_hdr() {
            return Err(format!("screen {} is not in HDR mode ({got:?})", i + 1));
        }
    }
    for w in watched {
        w.destroy();
    }
    for d in descriptions {
        d.destroy();
    }
    cm.destroy();
    Ok(())
}

struct HdrProbe {
    color: ColorData,
}

impl HasColor for HdrProbe {
    fn color(&mut self) -> &mut ColorData {
        &mut self.color
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for HdrProbe {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, ()> for HdrProbe {
    fn event(_: &mut Self, _: &WlOutput, _: <WlOutput as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

sctk::reexports::client::delegate_dispatch!(HdrProbe: [WpColorManagerV1: ()] => ColorData);
sctk::reexports::client::delegate_dispatch!(HdrProbe: [WpImageDescriptionV1: ()] => ColorData);
sctk::reexports::client::delegate_dispatch!(HdrProbe: [WpImageDescriptionInfoV1: ()] => ColorData);
sctk::reexports::client::delegate_dispatch!(HdrProbe: [WpImageDescriptionCreatorParamsV1: ()] => ColorData);
sctk::reexports::client::delegate_dispatch!(HdrProbe: [WpColorManagementSurfaceV1: ()] => ColorData);
sctk::reexports::client::delegate_dispatch!(HdrProbe: [WpColorManagementOutputV1: ()] => ColorData);
