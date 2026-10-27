// 远程窗口的网络 / 画面状态（用户 10-27 定，原型 dreamdesk/design/prototype/RemoteStats.html ①②③）：
// - 标题栏的信号格：按网络延时分三档（≤30 ms 三格绿、≤100 ms 两格橙、更高一格红，断线灰），悬停看延时，点了弹出详情卡；
// - 详情卡：实时 / 连接 / 画面三组「名称 / 值」，实时那组每秒刷新，没有数据的行不显示；
// - 画面右下角的质量监测（RustDesk 的 QualityMonitor 换成这个样子，开关照旧是 RustDesk 的 show_quality_monitor）。
// 数据：普通模式用 RustDesk 现成的——质量监测（延迟、帧率、速度、目标码率、编码、色度）、连接类型（TCP / UDP、直连 / 中继）、
// 对端信息（版本、系统、屏幕尺寸）；RustDesk 没交给界面的（双方 IP、被控端是不是硬编、采集方式、这次连接的时长）由 Rust 侧
// 通过现成的按会话通用查询给（key「dd-net-stats」，一个 JSON，src/ddpro/stats.rs），拿不到的行不显示。
// 质量监测面板的几行和专业窗口悬浮球的状态块同一套格式（src/ddpro/view.rs 的 status_lines），改一边要跟着改另一边
import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/shared_state.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/models/model.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:get/get.dart';

import 'dd_theme.dart';
import 'dd_titlebar_common.dart';

/// 一组「名称 / 值」
class _Row {
  final String label;
  final String value;
  const _Row(this.label, this.value);
}

/// 某一刻能拿到的全部数据；值为空的行不显示
class _Stats {
  double? delayMs;
  final realtime = <_Row>[];
  final connection = <_Row>[];
  final picture = <_Row>[];
}

/// 这次连接的时长「hh:mm:ss」（标题栏、质量监测面板都用它）；还没连上时为空串。
/// 起点在 Rust 侧（收到登录应答时，重连就重新算），和专业窗口悬浮球的时长是同一个
String ddElapsedText(FFI ffi) => _elapsedText(_extra(ffi));

String _elapsedText(Map<String, dynamic> extra) {
  final s = extra['connected_secs'];
  if (s is! int) return '';
  String two(int v) => v.toString().padLeft(2, '0');
  return '${two(s ~/ 3600)}:${two(s % 3600 ~/ 60)}:${two(s % 60)}';
}

/// 本机解码是硬是软：只取决于画面编码——RustDesk 对 H264 / H265 只用硬解（hwcodec / 显存解码），VP8 / VP9 / AV1 是软解
String? _decoderOf(String? codecFormat) => switch (codecFormat) {
      null || '' => null,
      'H264' || 'H265' => 'hw',
      _ => 'sw',
    };

/// Rust 侧补的数据（src/ddpro/stats.rs）：peer_ip、local_ip、encoder（"hw" / "sw"）、capture、connected_secs；
/// 拿不到的字段不给。这里只管通用模式：专业模式下通用窗口是藏起来的，专业通道的数据由专业窗口的悬浮球自己显示。
/// param 传这个窗口在看的那块屏（看「全部显示屏」时传空，Rust 侧取报过的第一块）
Map<String, dynamic> _extra(FFI ffi) {
  try {
    final d = ffi.ffiModel.pi.currentDisplay;
    final s = bind.sessionGetCommonSync(
        sessionId: ffi.sessionId, key: 'dd-net-stats', param: d == kAllDisplayValue ? '' : d.toString());
    if (s == null || s.isEmpty) return const {};
    final v = jsonDecode(s);
    return v is Map<String, dynamic> ? v : const {};
  } catch (e) {
    return const {};
  }
}

String _num(num v, {int digits = 1}) => v.toStringAsFixed(digits);

String _ms(num v) => v < 10 && v != v.roundToDouble() ? '${_num(v)} ms' : '${v.round()} ms';

String? _coder(Object? enc, Object? dec) {
  String? one(Object? v, String hw, String sw) => v == 'hw' ? translate(hw) : (v == 'sw' ? translate(sw) : null);
  final parts = [one(enc, 'Hardware encoding', 'Software encoding'), one(dec, 'Hardware decoding', 'Software decoding')]
      .whereType<String>()
      .toList();
  return parts.isEmpty ? null : parts.join(' / ');
}

// ---------------------------------------------------------------- 从 RustDesk 的数据里取值（详情卡和质量监测面板共用）

/// 网络延时（毫秒）：RustDesk 质量监测里的「延迟」，它隔几秒测一次往返
double? _delayOf(QualityMonitorData q) => double.tryParse(q.delay ?? '');

/// 带宽（千比特每秒）：RustDesk 给的速度是「12.34kB/s」（千字节）
double? _kbpsOf(QualityMonitorData q) {
  final kB = double.tryParse((q.speed ?? '').replaceAll('kB/s', ''));
  return kB == null ? null : kB * 1024 * 8 / 1000;
}

/// 通道：TCP / UDP、是不是直连；还没连上为空。照 RustDesk 标题栏盾牌的说明，中继走的是 TCP
({String stream, bool direct})? _channelOf(String id) {
  if (!Get.isRegistered<ConnectionType>(tag: ConnectionTypeState.tag(id))) return null;
  final t = ConnectionTypeState.find(id);
  if (!t.isValid()) return null;
  return (
    stream: t.stream_type.value == 'Relay' ? 'TCP' : t.stream_type.value.toUpperCase(),
    direct: t.direct.value == ConnectionType.strDirect,
  );
}

/// 这个窗口在看的那块屏（看「全部显示屏」时为空）
Display? _displayOf(FFI ffi) {
  final d = ffi.ffiModel.pi.tryGetDisplayIfNotAllDisplay();
  return d != null && d.width > 0 ? d : null;
}

_Stats _collect(FFI ffi, String id) {
  final extra = _extra(ffi);
  final q = ffi.qualityMonitorModel.data;
  final pi = ffi.ffiModel.pi;
  final s = _Stats()..delayMs = _delayOf(q);
  void add(List<_Row> to, String label, Object? value) {
    if (value is String && value.isNotEmpty) to.add(_Row(translate(label), value));
  }

  // 实时（帧延时、丢包率、无损比例只有专业模式有，通用模式不显示）
  add(s.realtime, 'Frame rate', q.fps == null ? null : '${q.fps} fps');
  final kbps = _kbpsOf(q);
  add(s.realtime, 'Bandwidth', kbps == null ? null : '${_num(kbps / 1000)} Mbps');
  // RustDesk 给的目标码率单位是 kb（每秒）
  final target = double.tryParse(q.targetBitrate ?? '');
  add(s.realtime, 'Target bitrate', target == null ? null : '${_num(target / 1000)} Mbps');

  // 连接
  final channel = _channelOf(id);
  add(s.connection, 'Secure channel',
      channel == null ? null : '${channel.stream} ${translate(channel.direct ? 'Direct' : 'Relayed')}');
  add(s.connection, 'Remote IP', extra['peer_ip']);
  add(s.connection, 'Local IP', extra['local_ip']);
  add(s.connection, 'Remote version', pi.version);
  // 完整系统名（如 Windows 11 Enterprise）来自网关的工作站信息，连之前记成这台机器的选项 dd-os（dd_home_page）；
  // 没有就用对端信息里的平台名（只有 Windows / Linux）
  final os = bind.mainGetPeerOptionSync(id: id, key: 'dd-os');
  add(s.connection, 'Remote OS', os.isNotEmpty ? os : pi.platform);

  // 画面
  final chroma = q.chroma == null || q.chroma == '-' ? '' : ' ${q.chroma}';
  final codec = q.codecFormat == null ? null : '${q.codecFormat}$chroma';
  final d = _displayOf(ffi);
  add(s.picture, 'Codec engine', _coder(extra['encoder'], _decoderOf(q.codecFormat)));
  add(s.picture, 'Encoding', codec);
  add(s.picture, 'Resolution', d == null ? null : '${d.width} × ${d.height}');
  add(s.picture, 'Capture method', extra['capture']);
  return s;
}

/// 质量监测面板的内容（原型 ③，照 UU「网络状态」）：一行一个数值、不写名称，单位都是英文。顺序：连接时长、通道、帧率、带宽、
/// 网络延时、分辨率档。帧延时、丢包率只有专业模式有（专业窗口悬浮球的状态块，src/ddpro/view.rs 的 status_lines，格式同这里）；
/// 拿不到的行不显示
List<String> _monitorLines(FFI ffi, String id) {
  final q = ffi.qualityMonitorModel.data;
  final channel = _channelOf(id);
  final kbps = _kbpsOf(q);
  final delay = _delayOf(q);
  final d = _displayOf(ffi);
  return [
    _elapsedText(_extra(ffi)),
    if (channel != null) '${channel.stream} ${channel.direct ? 'DIRECT' : 'RELAY'}',
    if (q.fps != null) '${q.fps} fps',
    // 不到 1 Mbps 写 Kbps
    if (kbps != null) kbps < 1000 ? '${_num(kbps)} Kbps' : '${_num(kbps / 1000)} Mbps',
    if (delay != null) _ms(delay),
    if (d != null)
      switch (d.width) {
        >= 3840 => '4K',
        >= 2560 => '2K',
        >= 1920 => '1080p',
        _ => '${d.width}×${d.height}',
      },
  ].where((l) => l.isNotEmpty).toList();
}

// ---------------------------------------------------------------- 信号格

enum _Level { good, fair, poor, offline }

_Level _levelOf(double? delay, bool offline) {
  if (offline || delay == null) return _Level.offline;
  if (delay <= 30) return _Level.good;
  if (delay <= 100) return _Level.fair;
  return _Level.poor;
}

/// 信号格：Lucide chart-no-axes-column-increasing 原图（三根竖线），按档位给颜色、把多出来的格淡掉（原型同样做法）
Widget _signal(_Level level, {double size = 16}) {
  final (color, lit) = switch (level) {
    _Level.good => (DdColors.online, 3),
    _Level.fair => (DdColors.busy, 2),
    _Level.poor => (DdColors.dangerText, 1),
    _Level.offline => (DdColors.offline, 1),
  };
  String line(int i, String x1, String y1) =>
      '<line x1="$x1" x2="$x1" y1="20" y2="$y1"${i > lit ? ' stroke-opacity=".25"' : ''}/>';
  final svg = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="3" '
      'stroke-linecap="round" stroke-linejoin="round">${line(1, '6', '16')}${line(2, '12', '10')}${line(3, '18', '4')}</svg>';
  return SvgPicture.string(svg, width: size, height: size, colorFilter: ColorFilter.mode(color, BlendMode.srcIn));
}

/// 标题栏上的信号格。网络延时 RustDesk 隔几秒测一次；画面数据每秒来一次，超过 4 秒没来当成断线（灰）
class DdSignal extends StatefulWidget {
  final String id;
  final FFI ffi;
  const DdSignal({super.key, required this.id, required this.ffi});

  @override
  State<DdSignal> createState() => _DdSignalState();
}

class _DdSignalState extends State<DdSignal> {
  DateTime? _lastData;
  Timer? _tick;
  final _menu = MenuController();
  late bool _show = widget.ffi.qualityMonitorModel.show;

  /// 质量监测模型一通知就算来了新数据——除了开 / 关质量监测面板那一下（它也通知，但不是数据；算进去的话断线后要多等 4 秒才变灰）。
  /// 不按值有没有变判断：画面静止时 RustDesk 每秒照样推一次，值可能一样，那也是连着的
  void _onData() {
    final show = widget.ffi.qualityMonitorModel.show;
    if (show != _show) {
      _show = show;
      return;
    }
    _lastData = DateTime.now();
  }

  @override
  void initState() {
    super.initState();
    widget.ffi.qualityMonitorModel.addListener(_onData);
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    widget.ffi.qualityMonitorModel.removeListener(_onData);
    _tick?.cancel();
    super.dispose();
  }

  bool get _offline {
    final t = _lastData;
    return t == null || DateTime.now().difference(t) > const Duration(seconds: 4);
  }

  @override
  Widget build(BuildContext context) {
    final delay = double.tryParse(widget.ffi.qualityMonitorModel.data.delay ?? '');
    final level = _levelOf(delay, _offline);
    final tip = level == _Level.offline || delay == null ? translate('No connection') : translate('Network latency {${_ms(delay)}}');
    return MenuAnchor(
      controller: _menu,
      alignmentOffset: const Offset(-12, 8),
      // 底色、圆角、阴影由全局的 menuTheme 给；详情卡自己留白，菜单不再加
      style: const MenuStyle(padding: WidgetStatePropertyAll(EdgeInsets.zero)),
      // 开 / 关时不重画：重画会拆掉正显示着的悬停提示，点开那一下出灰白出错块。开没开在点的时候问
      // （MenuController.isOpen 只是不能在画的时候问，第一次画时菜单还没挂上会抛错）
      menuChildren: [_NetDetails(id: widget.id, ffi: widget.ffi)],
      child: DdBarButton(
        tooltip: tip,
        onTap: () => _menu.isOpen ? _menu.close() : _menu.open(),
        child: _signal(level),
      ),
    );
  }
}

/// 点信号格弹出的详情卡（原型 ② dd-net）：宽 300；头一行信号格 +「网络延时：x ms」，下面实时 / 连接 / 画面三组两列。开着时每秒刷新
class _NetDetails extends StatefulWidget {
  final String id;
  final FFI ffi;
  const _NetDetails({required this.id, required this.ffi});

  @override
  State<_NetDetails> createState() => _NetDetailsState();
}

class _NetDetailsState extends State<_NetDetails> {
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final s = _collect(widget.ffi, widget.id);
    final groups = [s.realtime, s.connection, s.picture].where((g) => g.isNotEmpty).toList();
    final level = _levelOf(s.delayMs, false);
    Widget divider() => const Padding(
        padding: EdgeInsets.symmetric(horizontal: 20, vertical: 6), child: Divider(height: 1, color: Color(0xFFEEF0F3)));
    return SizedBox(
      width: 300,
      child: Padding(
        padding: const EdgeInsets.only(top: 14, bottom: 10),
        child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(20, 0, 20, 8),
            child: Row(children: [
              _signal(level),
              const SizedBox(width: 10),
              Text(
                  s.delayMs == null
                      ? translate('Network latency: -')
                      : translate('Network latency: {${_ms(s.delayMs!)}}'),
                  style: const TextStyle(fontSize: 15, fontWeight: FontWeight.w600, color: DdColors.ink)),
            ]),
          ),
          for (final g in groups) ...[
            divider(),
            for (final r in g)
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 20),
                child: SizedBox(
                  height: 28,
                  child: Row(children: [
                    SizedBox(
                        width: 96, child: Text(r.label, style: const TextStyle(fontSize: 13, color: DdColors.ink2))),
                    Flexible(
                      child: Text(r.value,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                              fontSize: 13, color: DdColors.ink, fontFeatures: [FontFeature.tabularFigures()])),
                    ),
                  ]),
                ),
              ),
          ],
        ]),
      ),
    );
  }
}

// ---------------------------------------------------------------- 质量监测

/// RustDesk 质量监测面板的受管版（原型 ③，照 UU「网络状态」）：黑底 70% 不透明、4 圆角，白色等宽字一行一个数值（[_monitorLines]）；
/// 显不显示照旧看 RustDesk 的开关。由 overlay.dart 的 QualityMonitor 一行钩子调进来，位置照旧（远程画面右下角）。
/// 跟着 RustDesk 每秒一次的画面数据刷新，连接时长也就每秒走一格
class DdQualityMonitor extends StatefulWidget {
  final QualityMonitorModel model;
  const DdQualityMonitor({super.key, required this.model});

  @override
  State<DdQualityMonitor> createState() => _DdQualityMonitorState();
}

class _DdQualityMonitorState extends State<DdQualityMonitor> {
  void _changed() {
    if (mounted) setState(() {});
  }

  @override
  void initState() {
    super.initState();
    widget.model.addListener(_changed);
  }

  @override
  void dispose() {
    widget.model.removeListener(_changed);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final ffi = widget.model.parent.target;
    if (!widget.model.show || ffi == null) return const SizedBox.shrink();
    final lines = _monitorLines(ffi, ffi.id);
    if (lines.isEmpty) return const SizedBox.shrink();
    // 等宽字：Windows 有 Consolas，Linux 退到 DejaVu Sans Mono，都没有交给系统的等宽字体
    const style = TextStyle(
        fontFamily: 'Consolas',
        fontFamilyFallback: ['DejaVu Sans Mono', 'Liberation Mono', 'monospace'],
        fontSize: 12,
        height: 16 / 12,
        color: Colors.white);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      decoration: BoxDecoration(color: const Color(0xB3000000), borderRadius: BorderRadius.circular(DdRadius.sm)),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [for (final l in lines) Text(l, style: style)],
      ),
    );
  }
}
