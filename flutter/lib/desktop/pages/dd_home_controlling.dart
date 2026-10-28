// 首页「本机正在控制这台」（原型 HomeControlling 收起 / HomeControllingOpen 展开）：左栏会话数角标、详情页的蓝色横条（展开是正在连接的
// 设备列表）、「进入桌面」换成「已建立远程连接 + 显示远控窗口」。会话从哪来见 dd_local_sessions.dart。从 dd_home_page.dart 拆出来的
part of 'dd_home_page.dart';

/// 左栏设备名右边的会话数角标：蓝框、人形图标 + 数字
class _SessionBadge extends StatelessWidget {
  final int count;
  const _SessionBadge({required this.count});

  @override
  Widget build(BuildContext context) => Container(
        height: 18,
        margin: const EdgeInsets.only(left: 6),
        padding: const EdgeInsets.symmetric(horizontal: 5),
        decoration: BoxDecoration(
            border: Border.all(color: DdColors.controlBadge), borderRadius: BorderRadius.circular(DdRadius.sm)),
        child: Row(mainAxisSize: MainAxisSize.min, children: [
          ddIcon(DdIcon.user, size: 11, color: DdColors.accent),
          const SizedBox(width: 2),
          Text('$count', style: const TextStyle(fontSize: 11, color: DdColors.accent)),
        ]),
      );
}

/// 本机这头的信息：会话列表每一行写的是本机（设备名 · 当前登录的网关账号 + 「本机」）
class _LocalDevice {
  final String name;
  final String os;
  const _LocalDevice(this.name, this.os);

  static _LocalDevice read() {
    try {
      final m = jsonDecode(bind.mainGetLoginDeviceInfo());
      if (m is Map) return _LocalDevice('${m['name'] ?? ''}', '${m['os'] ?? ''}');
    } catch (e) {
      debugPrint('dd-home: login device info: $e');
    }
    return const _LocalDevice('', '');
  }
}

/// 详情页标题下的蓝色横条：「被控中 | 本机正在控制此电脑」、「断开所有远控」、展开箭头；展开后下面是正在连接的设备
class _ControlBanner extends StatelessWidget {
  final List<DdLocalSession> sessions;
  final bool open;
  final VoidCallback onToggle;
  final _LocalDevice local;
  final String account;
  const _ControlBanner(
      {required this.sessions, required this.open, required this.onToggle, required this.local, required this.account});

  static ButtonStyle get _buttonStyle => OutlinedButton.styleFrom(
        minimumSize: const Size(0, 28),
        padding: const EdgeInsets.symmetric(horizontal: 12),
        backgroundColor: DdColors.surface,
        foregroundColor: DdColors.ink,
        side: const BorderSide(color: DdColors.line),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(DdRadius.sm)),
        textStyle: const TextStyle(fontSize: 13),
        tapTargetSize: MaterialTapTargetSize.shrinkWrap,
      );

  @override
  Widget build(BuildContext context) {
    final header = Material(
      color: DdColors.controlBg,
      child: InkWell(
        onTap: onToggle,
        hoverColor: Colors.transparent,
        child: Container(
          height: 52,
          padding: const EdgeInsets.only(left: 18, right: 16),
          child: Row(children: [
            ddIcon(DdIcon.screenShare, size: 20, color: DdColors.controlText),
            const SizedBox(width: 10),
            Text(translate('Being controlled'),
                style: const TextStyle(fontSize: 15, fontWeight: FontWeight.w700, color: DdColors.controlText)),
            const SizedBox(width: 10),
            Container(width: 1, height: 14, color: DdColors.controlDivider),
            const SizedBox(width: 10),
            Expanded(
              child: Text(translate('This computer is controlling it'),
                  overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 13, color: DdColors.controlSub)),
            ),
            OutlinedButton(
              style: _buttonStyle,
              onPressed: () async {
                for (final s in sessions) {
                  await ddCloseSession(s);
                }
              },
              child: Text(translate('Disconnect all remote controls')),
            ),
            const SizedBox(width: 10),
            ddIcon(open ? DdIcon.chevronUp : DdIcon.chevronDown, size: 18, color: DdColors.controlText),
          ]),
        ),
      ),
    );
    if (!open) {
      return ClipRRect(borderRadius: BorderRadius.circular(DdRadius.lg), child: header);
    }
    return Container(
      decoration: BoxDecoration(
        color: DdColors.surface,
        border: Border.all(color: DdColors.controlBorder),
        borderRadius: BorderRadius.circular(DdRadius.lg),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        header,
        Padding(
          padding: const EdgeInsets.fromLTRB(18, 12, 18, 8),
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            Text(translate('Connected devices'), style: const TextStyle(fontSize: 12, color: DdColors.ink2)),
            for (final s in sessions) _sessionRow(s),
          ]),
        ),
      ]),
    );
  }

  Widget _sessionRow(DdLocalSession s) {
    final name = local.name.toUpperCase();
    return SizedBox(
      height: 64,
      child: Row(children: [
        _OsBadge(os: local.os, online: true, size: 36),
        const SizedBox(width: 14),
        Expanded(
          child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
            Row(children: [
              Flexible(
                child: Text.rich(
                  TextSpan(children: [
                    TextSpan(text: name),
                    if (account.isNotEmpty) TextSpan(text: ' · $account', style: const TextStyle(color: DdColors.ink2)),
                  ]),
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(fontSize: 14, color: DdColors.ink),
                ),
              ),
              DdTag(text: translate('This computer')),
            ]),
            const SizedBox(height: 4),
            Row(children: [
              Container(
                height: 18,
                padding: const EdgeInsets.symmetric(horizontal: 4),
                alignment: Alignment.center,
                decoration: BoxDecoration(
                    border: Border.all(color: DdColors.tagBorder), borderRadius: BorderRadius.circular(3)),
                child: Text(s.label, style: const TextStyle(fontSize: 12, color: DdColors.ink2)),
              ),
              const SizedBox(width: 6),
              _Elapsed(since: s.startedAt),
            ]),
          ]),
        ),
        OutlinedButton(style: _buttonStyle, onPressed: () => ddCloseSession(s), child: Text(translate('Disconnect remote control'))),
      ]),
    );
  }
}

/// 「连接时长 00:00:20」，每秒走一次（只重画这一小段，不动整个首页）
class _Elapsed extends StatefulWidget {
  final DateTime since;
  const _Elapsed({required this.since});

  @override
  State<_Elapsed> createState() => _ElapsedState();
}

class _ElapsedState extends State<_Elapsed> {
  late final Timer _timer;

  @override
  void initState() {
    super.initState();
    _timer = Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}));
  }

  @override
  void dispose() {
    _timer.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Text(
        '${translate('Connection time')} ${ddDurationText(DateTime.now().difference(widget.since))}',
        style: const TextStyle(fontSize: 12, color: DdColors.ink2),
      );
}

/// 详情大卡片的上半，本机正连着这台时：模糊的壁纸、中间大图标「已建立远程连接」和「显示远控窗口」
class _ControllingHero extends StatelessWidget {
  final String os;
  final VoidCallback onShow;
  const _ControllingHero({required this.os, required this.onShow});

  @override
  Widget build(BuildContext context) {
    // Stack 默认按自己的大小裁边
    return Stack(fit: StackFit.expand, children: [
      // 原型：四周外扩 12 再模糊、压暗一点，边上不露出模糊带进来的透明边
      Positioned(
        left: -12,
        right: -12,
        top: -12,
        bottom: -12,
        child: ImageFiltered(
          imageFilter: ui.ImageFilter.blur(sigmaX: 10, sigmaY: 10),
          child: ColorFiltered(
            colorFilter: const ColorFilter.matrix([
              0.85, 0, 0, 0, 0, //
              0, 0.85, 0, 0, 0, //
              0, 0, 0.85, 0, 0, //
              0, 0, 0, 1, 0,
            ]),
            child: _OsWallpaper(os: os, online: true),
          ),
        ),
      ),
      Center(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          ddIcon(DdIcon.screenShare, size: 56, color: DdColors.onAccent),
          const SizedBox(height: 12),
          Text(translate('Remote connection established'),
              style: const TextStyle(fontSize: 13, color: DdColors.onAccent)),
          const SizedBox(height: 12),
          ElevatedButton(
            onPressed: onShow,
            style: ElevatedButton.styleFrom(
              minimumSize: const Size(0, 32),
              padding: const EdgeInsets.symmetric(horizontal: 18),
              textStyle: const TextStyle(fontSize: 13),
            ),
            child: Text(translate('Show remote window')),
          ),
        ]),
      ),
    ]);
  }
}
