// DreamDesk 受管版的主窗口首页（dreamdesk/docs/M-受管版方案.md「桌面端首页」，用户 10-26 定：照网易 UU 远程主窗口的布局和样式）：
// 浅灰的标题栏和左栏连成一片，右边更浅的内容区左上角是圆角。左栏「我的设备」列出网关授权给我的工作站（/api/dd/workstations，
// 带在线、在用）和「全部设备」，左下角「设置」；右边没登录时是插画 +「立即登录」，登录后是全部设备，或一台的详情
// （「进入桌面」大卡片 + 文件传输、终端、端口映射）。只连受管工作站，没有按设备号连接（「远程协助」）。
// 登录框按 UU 的样子另画（只有域账号口令，认证由网关向 AD 做）；设置页就是 RustDesk 的设置页。
// RustDesk 原来的首页（DesktopTabPage）照样建起来、只是不显示：远程窗口要它应答主窗口的那套请求（屏幕列表、窗口激活登记等），
// 设置页也要它登记的东西。
import 'dart:async';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart' show listEquals;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hbb/common.dart' hide Dialog;
import 'package:flutter_hbb/desktop/pages/desktop_setting_page.dart';
import 'package:flutter_hbb/desktop/pages/desktop_tab_page.dart';
import 'package:flutter_hbb/desktop/widgets/dd_icons.dart';
import 'package:flutter_hbb/desktop/widgets/dd_titlebar_common.dart';
import 'package:flutter_hbb/desktop/widgets/dd_restyle.dart';
import 'package:flutter_hbb/desktop/widgets/dd_theme.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/state_model.dart';
import 'package:flutter_hbb/utils/dd_gateway.dart';
import 'package:get/get.dart';
import 'package:window_manager/window_manager.dart';

part 'dd_home_login.dart';
part 'dd_home_title_bar.dart';
part 'dd_home_widgets.dart';

/// 机器名一律大写显示（照 UU；远程窗口标题栏也是大写）。只在显示时转，存的（备注名）是网关给的原样
String _title(DdWs w) => w.name.toUpperCase();

/// 右边显示什么：全部设备、某一台（设备号）、设置
const _kAll = '';
const _kSettings = '\u0000settings';
const _kListRefresh = Duration(seconds: 15);
/// 本机记住的上一个登录名（只记用户名，口令不落地）
const _kLastUser = 'dd-last-user';

/// 单击马上生效，同一个目标在系统双击间隔内再点一次算双击。不用 onDoubleTap：有它在，单击要等约 300 毫秒确认不是双击才生效
/// （点设备显得慢），在标题栏上还会和拖动抢鼠标（拖不动）；RustDesk 标签栏也是这样自己判断双击的
class _Clicks {
  static Object? _target;
  static int _at = 0;

  static bool isDouble(Object target) {
    final now = DateTime.now().millisecondsSinceEpoch;
    final dbl = target == _target && now - _at < bind.getDoubleClickTime();
    _target = dbl ? null : target;
    _at = now;
    return dbl;
  }
}

class DdHomePage extends StatefulWidget {
  const DdHomePage({Key? key}) : super(key: key);

  @override
  State<DdHomePage> createState() => _DdHomePageState();
}

class _DdHomePageState extends State<DdHomePage> with WidgetsBindingObserver {
  String _selected = _kAll;

  /// 打开（或重新登录）后第一次拿到列表时选中第一台（照 UU）；之后用户点了「全部设备」就不再替他选
  bool _pickFirst = true;
  SettingsTabKey _settingsTab = SettingsTabKey.general;
  bool _devicesOpen = true;
  List<DdWs> _items = [];
  bool _loaded = false;

  /// 上一次取列表没成的原因（成功就清掉）。还没有列表时整页报错；已经有列表时留着旧列表、右下角挂一条提示（build 里按 [_loaded] 分）
  DdFailure? _failure;

  /// 服务器回的说明（[_failure] 是 server 时可能有）
  String _failMessage = '';

  /// 令牌被拒、登录被清掉了：没登录时的页面写「登录已过期」，而不是普通的登录提示
  bool _expired = false;
  Timer? _timer;
  late final Worker _loginWorker;
  /// 有没有登录（本机存着令牌）。存成字段、只在登录状态变化和每次刷新时读一次配置，不在每次 build 里同步问 FFI
  late bool _signedIn = _readSignedIn();
  /// 上一次请求还在路上：定时器到点时跳过，不叠请求
  bool _fetching = false;
  /// 窗口藏起来（最小化、收到托盘）：不刷新；回来时马上刷一次
  bool _hidden = false;

  static bool _readSignedIn() => bind.mainGetLocalOption(key: 'access_token').isNotEmpty;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    // 登录 / 退出（不管在哪登的：这里、设置页的账户）：用户名一变就按令牌对一次
    _loginWorker = ever(gFFI.userModel.userName, (_) => _syncSignedIn());
    _timer = Timer.periodic(_kListRefresh, (_) {
      if (!_hidden) _fetch();
    });
    WidgetsBinding.instance.addPostFrameCallback((_) => _fetch());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _loginWorker.dispose();
    _timer?.cancel();
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    final hidden = state == AppLifecycleState.hidden || state == AppLifecycleState.paused;
    if (hidden == _hidden) return;
    _hidden = hidden;
    if (!hidden) _fetch();
  }

  /// 登录状态以本机令牌为准，只在这一处对：变了就清掉上一个人的列表、重新取。用户名变了（RustDesk 的登录 / 退出、
  /// 这里的登录框 [ddSignIn] 存好令牌后）、令牌被拒、每次取列表前都调它。受管版没有设置页「账户」：RustDesk 原登录框
  /// 是先设用户名、后存令牌，从那里登这一下会看不到令牌
  void _syncSignedIn() {
    if (!mounted) return;
    final signedIn = _readSignedIn();
    if (signedIn == _signedIn) return;
    setState(() {
      _signedIn = signedIn;
      _items = [];
      _loaded = false;
      _failure = null;
      // 「登录已过期」留到重新登录为止（令牌被拒时登录状态先被清掉，这里会跟着走一遍）
      if (signedIn) _expired = false;
      if (_selected != _kSettings) _selected = _kAll;
      _pickFirst = true;
    });
    if (signedIn) _fetch(force: true);
  }

  void _retry() {
    setState(() {
      _failure = null;
      _loaded = false;
    });
    _fetch(force: true);
  }

  int _fetchSeq = 0;

  /// 取一次列表。上一次还在路上就跳过（定时器、回到前台触发的）；[force]（换人、点「再试」）不跳过，
  /// 这时更早的那次回来会发现自己不是最新的一次、丢掉结果（[_load]）
  Future<void> _fetch({bool force = false}) async {
    if (_fetching && !force) return;
    final seq = ++_fetchSeq;
    _fetching = true;
    try {
      await _load(seq);
    } finally {
      if (seq == _fetchSeq) _fetching = false;
    }
  }

  Future<void> _load(int seq) async {
    _syncSignedIn();
    final token = bind.mainGetLocalOption(key: 'access_token');
    if (token.isEmpty) return;
    final r = await ddFetchWorkstations();
    // 晚回来的不要：后面又发了一次（点「再试」、换人），或者请求在路上时换了人 / 退出了（不然会显示上一个人的工作站，
    // 或者拿上一个人已注销的令牌被拒、把刚登录的人踢下线）
    if (!mounted || seq != _fetchSeq || bind.mainGetLocalOption(key: 'access_token') != token) return;
    if (r.rejected) {
      setState(() => _expired = true);
      await gFFI.userModel.reset(resetOther: true);
      _syncSignedIn();
      return;
    }
    final items = r.items;
    if (items == null) {
      // 已经有列表时留着旧的、挂提示条（build 里按 _loaded 分）；还没有列表才整页报错
      debugPrint('dd-home: listing workstations failed: ${r.failure}');
      if (_failure != r.failure || _failMessage != r.message) {
        setState(() {
          _failure = r.failure;
          _failMessage = r.message;
        });
      }
      return;
    }
    var selected = _selected;
    if (_pickFirst && selected == _kAll && items.isNotEmpty) selected = items.first.id;
    // 选中的那台不在授权里了：回到全部设备
    if (selected != _kAll && selected != _kSettings && !items.any((w) => w.id == selected)) selected = _kAll;
    // 什么都没变就不重画（每 15 秒一次，整个首页连设置页都要重建一遍）；提示条还挂着时要收掉，不算「没变」
    if (_loaded && _failure == null && !_pickFirst && selected == _selected && listEquals(items, _items)) return;
    setState(() {
      _items = items;
      _loaded = true;
      _failure = null;
      _selected = selected;
      _pickFirst = false;
    });
  }

  void _openSettings(SettingsTabKey tab) => setState(() {
        _selected = _kSettings;
        _settingsTab = tab;
      });

  Future<void> _connect(DdWs w, {bool fileTransfer = false, bool terminal = false, bool tunnel = false}) async {
    if (!w.selectable) return;
    // 远程窗口左上角写的是这台机器的备注名（RustDesk 的 alias）：先记成网关给的工作站名，和用户在列表里点的一致。
    // 网关没给名字时列表里显示的是设备号，那就不记（左上角照旧取主机名）
    if (w.named && bind.mainGetPeerOptionSync(id: w.id, key: 'alias') != w.name) {
      await bind.mainSetPeerAlias(id: w.id, alias: w.name);
    }
    // 信号格详情里「被控端 IP」「被控端系统」：连接里查不到，用网关给的，连前记成对端选项（没给的记空，等于清掉上次的）
    for (final (key, value) in [('dd-ip', w.info['ip'] ?? ''), ('dd-os', ddOsShort(w.os))]) {
      if (bind.mainGetPeerOptionSync(id: w.id, key: key) != value) {
        await bind.mainSetPeerOption(id: w.id, key: key, value: value);
      }
    }
    if (!mounted) return;
    connect(context, w.id, isFileTransfer: fileTransfer, isTerminal: terminal, isTcpTunneling: tunnel);
  }

  @override
  Widget build(BuildContext context) {
    final page = Stack(children: [
      const Offstage(child: DesktopTabPage()),
      // 受管版整个应用固定浅色、用 ddLightTheme（main.dart），点击没有水波纹也在那个主题里；这里不再另套主题
      Material(
        color: DdColors.chrome,
        child: DefaultTextStyle.merge(
          style: const TextStyle(color: DdColors.ink),
          child: Column(children: [
            _TitleBar(onAbout: () => _openSettings(SettingsTabKey.about)),
            Expanded(
              child: Row(children: [
                  SizedBox(width: DdSize.sidebarWidth, child: _sidebar()),
                  // 内容区照 UU：上边、左边一条细线和左栏分开，左上角圆角（外层是线的颜色，里层错开 1 像素铺内容区的底）
                  Expanded(
                    child: Container(
                      padding: const EdgeInsets.only(left: 1, top: 1),
                      decoration: const BoxDecoration(
                        color: DdColors.paneLine,
                        borderRadius: BorderRadius.only(topLeft: Radius.circular(DdRadius.xl)),
                      ),
                      child: ClipRRect(
                        borderRadius: const BorderRadius.only(topLeft: Radius.circular(DdRadius.xl - 1)),
                        child: Container(
                          color: DdColors.pane,
                          child: Stack(children: [
                            Positioned.fill(child: _right()),
                            // 已经有列表、后台这次刷新没成：留着旧列表挂提示条（还没有列表时是整页报错）。
                            // 提示的是设备列表，设置页里不挂；没登录时没有列表
                            if (_failure != null && _loaded && _signedIn && _selected != _kSettings)
                              Positioned(
                                right: 24,
                                bottom: 18,
                                child: _NetWarn(
                                  text: translate(_failure == DdFailure.network
                                      ? 'Cannot reach the server. The list may be out of date.'
                                      : 'The server returned an error. The list may be out of date.'),
                                  onRetry: () => _fetch(force: true),
                                ),
                              ),
                          ]),
                        ),
                      ),
                    ),
                  ),
                ]),
            ),
          ]),
        ),
      ),
    ]);
    // 无边框窗口靠这一层拖边缩放（和 RustDesk 主窗口原来的首页一样）
    return isMacOS || kUseCompatibleUiMode
        ? page
        : Obx(() => DragToResizeArea(
              resizeEdgeSize: stateGlobal.resizeEdgeSize.value,
              enableResizeEdges: windowManagerEnableResizeEdges,
              child: page,
            ));
  }

  Widget _sidebar() {
    final signedIn = _signedIn;
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      const SizedBox(height: 6),
      _SideRow(
        icon: ddIcon(DdIcon.devices),
        label: translate('My devices'),
        trailing: ddIcon(_devicesOpen ? DdIcon.chevronUp : DdIcon.chevronDown, size: 16, color: DdColors.ink2),
        onTap: () => setState(() => _devicesOpen = !_devicesOpen),
      ),
      Expanded(
        child: ListView(padding: EdgeInsets.zero, children: [
          if (_devicesOpen) ...[
            if (signedIn)
              for (final w in _items)
                _SideRow(
                  indent: true,
                  dot: w.online ? DdColors.online : null,
                  icon: _OsBadge(os: w.os, online: w.online, size: 20),
                  label: _title(w),
                  dim: !w.online,
                  selected: _selected == w.id,
                  onTap: () => setState(() => _selected = w.id),
                  onDoubleTap: () => _connect(w),
                  clickId: w.id,
                ),
            _SideRow(
              indent: true,
              icon: ddAllDevicesIcon(),
              label: translate('All devices'),
              selected: _selected == _kAll,
              // 用户自己点了「全部设备」：列表回来后不再替他选中第一台
              onTap: () => setState(() {
                _selected = _kAll;
                _pickFirst = false;
              }),
            ),
          ],
        ]),
      ),
      // 「设置」上方一条横贯左栏的分隔线（照 UU）
      Container(height: 1, color: DdColors.sideDivider),
      const SizedBox(height: 6),
      _SideRow(
        icon: ddIcon(DdIcon.settings),
        label: translate('Settings'),
        selected: _selected == _kSettings,
        onTap: () => _openSettings(SettingsTabKey.general),
      ),
      const SizedBox(height: 10),
    ]);
  }

  Widget _right() {
    if (_selected == _kSettings) {
      // 换了要打开的标签（例如从「更多 → 关于」进来）时重建，设置页只在建起来时看 initialTabkey
      return DesktopSettingPage(key: ValueKey(_settingsTab), initialTabkey: _settingsTab);
    }
    if (!_signedIn) return _signInPrompt();
    final w = _items.where((w) => w.id == _selected).firstOrNull;
    return w == null ? _allDevices() : _detail(w);
  }

  Widget _page({required Widget title, required Widget child}) => Padding(
        padding: const EdgeInsets.fromLTRB(40, 30, 40, 24),
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          title,
          const SizedBox(height: 22),
          Expanded(child: child),
        ]),
      );

  Widget _heading(String text) => Text(text,
      maxLines: 1, overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 24, fontWeight: FontWeight.w700, color: DdColors.ink));

  Widget _signInPrompt() {
    return _page(
      title: _heading(translate('All devices')),
      child: _expired
          ? _StateView(
              icon: ddIcon(DdIcon.lock, size: 40, color: DdColors.ink3),
              title: translate('Your sign-in has expired. Please sign in again.'),
              action: ElevatedButton(
                  onPressed: () => _showLogin(context), style: ddCompactButton, child: Text(translate('Sign in again'))),
            )
          // 窗口矮时放不下插画 + 文字 + 按钮：居中，放不下就能往上滚，不溢出
          : Center(
              child: SingleChildScrollView(
                child: Column(mainAxisSize: MainAxisSize.min, children: [
                  const _Illustration(),
                  const SizedBox(height: 26),
                  Text(translate('Sign in to see the workstations assigned to you.'),
                      style: const TextStyle(fontSize: 15, color: DdColors.ink)),
                  const SizedBox(height: 18),
                  ElevatedButton(onPressed: () => _showLogin(context), child: Text(translate('Sign in now'))),
                ]),
              ),
            ),
    );
  }

  Widget _allDevices() {
    final Widget body;
    // 加载中 / 出错 / 没有授权的工作站：照原型 HomeStates，图标 + 一句话（+ 说明）（+ 按钮）
    final failure = _loaded ? null : _failure;
    if (failure != null) {
      final net = failure == DdFailure.network;
      body = _StateView(
        icon: ddIcon(net ? DdIcon.wifiOff : DdIcon.circleAlert, size: 40, color: DdColors.ink3),
        title: net
            ? translate('Cannot reach the server')
            : (_failMessage.isNotEmpty ? _failMessage : translate('Could not load your workstations')),
        subtitle: net ? translate('Check the network and try again') : null,
        action: OutlinedButton(onPressed: _retry, style: ddCompactButton, child: Text(translate('Retry'))),
      );
    } else if (!_loaded) {
      body = _StateView(
        icon: const SizedBox(
            width: 24, height: 24, child: CircularProgressIndicator(strokeWidth: 2, color: DdColors.accent)),
        title: translate('Loading...'),
        dim: true,
      );
    } else if (_items.isEmpty) {
      body = _StateView(
        icon: ddIcon(DdIcon.monitor, size: 40, color: DdColors.ink3),
        title: translate('No workstation has been assigned to you yet'),
        subtitle: translate('Please contact IT'),
      );
    } else {
      body = SingleChildScrollView(
        child: Wrap(spacing: 16, runSpacing: 16, children: [
          for (final w in _items)
            _DeviceTile(
              ws: w,
              stateText: _stateText(w.state),
              stateColor: _stateColor(w.state),
              onTap: () => setState(() => _selected = w.id),
            ),
        ]),
      );
    }
    return _page(
      // 标题后面写台数（照原型）
      title: Row(crossAxisAlignment: CrossAxisAlignment.baseline, textBaseline: TextBaseline.alphabetic, children: [
        _heading(translate('All devices')),
        if (_loaded && _failure == null && _items.isNotEmpty) ...[
          const SizedBox(width: 8),
          // 英文写成「1 in total」，不用分单复数
          Text(translate('{${_items.length}} in total'), style: const TextStyle(fontSize: 13, color: DdColors.ink2)),
        ],
      ]),
      child: body,
    );
  }

  Widget _detail(DdWs w) {
    final on = w.selectable;
    final actions = [
      (DdIcon.folderUp, translate('File transfer'), () => _connect(w, fileTransfer: true)),
      (DdIcon.terminal, translate('Terminal'), () => _connect(w, terminal: true)),
      (DdIcon.arrowLeftRight, translate('Port mapping'), () => _connect(w, tunnel: true)),
    ];
    return _page(
      title: Row(children: [
        _StatePill(text: _stateText(w.state), color: _stateColor(w.state)),
        const SizedBox(width: 12),
        // 名字后面灰字写系统，「更多」按钮靠右、浅灰底（照原型）
        Expanded(
          child: Row(children: [
            Flexible(child: _heading(_title(w))),
            if (ddOsShort(w.os).isNotEmpty) ...[
              const SizedBox(width: 12),
              Text(ddOsShort(w.os), style: const TextStyle(fontSize: 12, color: DdColors.ink2)),
            ],
          ]),
        ),
        // 浅灰底的圆角方块（照原型）。PopupMenuButton 自带的 InkWell 把悬停底色画在最近的 Material 上：
        // 浅灰底在外、里面垫一层透明 Material，悬停时才看得见再深一档；菜单的底、圆角、阴影用主题的 popupMenuTheme
        Container(
          decoration: BoxDecoration(color: DdColors.hover, borderRadius: BorderRadius.circular(DdRadius.md)),
          clipBehavior: Clip.antiAlias,
          child: Material(
            type: MaterialType.transparency,
            child: PopupMenuButton<int>(
          tooltip: '',
          offset: const Offset(0, 38),
          child: SizedBox(width: 32, height: 32, child: Center(child: ddIcon(DdIcon.moreVertical))),
          itemBuilder: (_) => [
            for (final (i, a) in actions.indexed)
              PopupMenuItem<int>(
                value: i,
                enabled: on,
                height: 36,
                child: Row(children: [
                  ddIcon(a.$1, color: on ? DdColors.ink : DdColors.ink3),
                  const SizedBox(width: 10),
                  Text(a.$2, style: const TextStyle(fontSize: 14)),
                ]),
              ),
          ],
          onSelected: (i) => actions[i].$3(),
            ),
          ),
        ),
      ]),
      child: SingleChildScrollView(
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          // 一张卡片：上面是「进入桌面」（能用专业模式时自动是专业模式），下面一条白色的操作栏。
          // 白底用 Material：操作栏格子的悬停底色画在最近的 Material 上，白底是普通 Container 的话会被它挡住、看不见
          Container(
            decoration: BoxDecoration(borderRadius: BorderRadius.circular(DdRadius.lg), boxShadow: DdShadow.card),
            child: Material(
              color: DdColors.surface,
              borderRadius: BorderRadius.circular(DdRadius.lg),
              clipBehavior: Clip.antiAlias,
              child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              SizedBox(
                height: 220,
                child: _EnterDesktop(
                  label: on ? translate('Enter desktop') : _stateText(w.state),
                  arrow: on,
                  os: w.os,
                  online: w.online,
                  onTap: on ? () => _connect(w) : null,
                ),
              ),
              SizedBox(
                height: 52,
                child: Row(children: [
                  for (final (i, a) in actions.indexed) ...[
                    if (i > 0) Container(width: 1, height: 18, color: DdColors.line),
                    Expanded(child: _ActionCell(icon: a.$1, label: a.$2, onTap: on ? a.$3 : null)),
                  ],
                ]),
              ),
            ]),
            ),
          ),
          const SizedBox(height: 18),
          _DeviceInfo(ws: w),
        ]),
      ),
    );
  }

  String _stateText(DdWsState s) => translate(switch (s) {
        DdWsState.available => 'Online',
        DdWsState.mine => 'Connected from elsewhere',
        DdWsState.busy => 'In use by someone else',
        DdWsState.offline => 'Offline',
      });

  Color _stateColor(DdWsState s) => switch (s) {
        DdWsState.available => DdColors.online,
        DdWsState.mine => DdColors.mine,
        DdWsState.busy => DdColors.busy,
        DdWsState.offline => DdColors.ink3,
      };
}
