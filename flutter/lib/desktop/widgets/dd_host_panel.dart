// 受管版被控端的「本机被控中」面板（用户 10-28 定，照 UU；原型 HostPanel.html 展开、HostCollapsed.html 收起）。
// 数据和状态都在 dd_host_model.dart 的 [ddHost]。首页这样接：左栏顶上无条件放 [DdHostCard]（只在「有连接且已收起」时显示）；
// `ddHost.expanded && ddHost.hasClients` 时标题栏以下整块换成 [DdHostPanel]（不要左栏）。
import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';

import 'dd_host_model.dart';
import 'dd_icons.dart';
import 'dd_theme.dart';
import 'dd_titlebar_common.dart';

String _clock(int seconds) {
  final s = seconds < 0 ? 0 : seconds;
  String two(int v) => v.toString().padLeft(2, '0');
  return '${two(s ~/ 3600)}:${two(s % 3600 ~/ 60)}:${two(s % 60)}';
}

int _now() => DateTime.now().millisecondsSinceEpoch ~/ 1000;

String _countText(int n) => translate('{} device(s) controlling this computer').replaceFirst('{}', '$n');

/// 展开的大面板：上面蓝色横幅（计时、人数、断开所有），下面是正在连接的设备列表
class DdHostPanel extends StatelessWidget {
  const DdHostPanel({super.key});

  @override
  Widget build(BuildContext context) => ListenableBuilder(
        listenable: ddHost,
        builder: (context, _) {
          final since = ddHost.since;
          return ColoredBox(
            color: DdColors.pane,
            child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              Container(
                height: 160,
                margin: const EdgeInsets.fromLTRB(12, 8, 12, 0),
                clipBehavior: Clip.antiAlias,
                decoration: BoxDecoration(color: DdColors.hostBlue, borderRadius: BorderRadius.circular(DdRadius.lg)),
                child: Stack(children: [
                  Positioned(
                    right: 70,
                    top: 18,
                    child: ddIcon(DdIcon.screenShare, size: 180, strokeWidth: 1.2, color: DdColors.onAccent.withAlpha(31)),
                  ),
                  Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 40),
                    child: Row(children: [
                      Expanded(
                        child: Column(mainAxisAlignment: MainAxisAlignment.center, crossAxisAlignment: CrossAxisAlignment.start, children: [
                          Row(crossAxisAlignment: CrossAxisAlignment.baseline, textBaseline: TextBaseline.alphabetic, children: [
                            Text(translate('This computer is being controlled'),
                                style: const TextStyle(fontSize: 28, fontWeight: FontWeight.w700, color: DdColors.onAccent)),
                            const SizedBox(width: 28),
                            Text(_clock(since == 0 ? 0 : _now() - since),
                                style: const TextStyle(
                                    fontSize: 28,
                                    fontWeight: FontWeight.w700,
                                    color: DdColors.onAccent,
                                    fontFeatures: [FontFeature.tabularFigures()])),
                          ]),
                          const SizedBox(height: 10),
                          Text(_countText(ddHost.count),
                              style: TextStyle(fontSize: 14, color: DdColors.onAccent.withAlpha(230))),
                        ]),
                      ),
                      _OutlineButton(text: translate('Disconnect all remote controls'), onTap: ddHost.disconnect),
                    ]),
                  ),
                  Positioned(
                    right: 14,
                    top: 12,
                    child: _IconTap(
                      icon: DdIcon.minimize2,
                      size: 18,
                      color: DdColors.onAccent,
                      onTap: () => ddHost.expanded = false,
                    ),
                  ),
                ]),
              ),
              Expanded(
                child: SingleChildScrollView(
                  padding: const EdgeInsets.fromLTRB(40, 28, 40, 28),
                  child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                    Text(translate('Connected devices'), style: const TextStyle(fontSize: 13, color: DdColors.ink)),
                    const SizedBox(height: 10),
                    for (final c in ddHost.clients) _ClientRow(client: c),
                  ]),
                ),
              ),
            ]),
          );
        },
      );
}

/// 收起时左栏最上面的蓝色小卡片
class DdHostCard extends StatelessWidget {
  const DdHostCard({super.key});

  @override
  Widget build(BuildContext context) => ListenableBuilder(
        listenable: ddHost,
        builder: (context, _) {
          if (!ddHost.hasClients || ddHost.expanded) return const SizedBox.shrink();
          return Container(
            height: 88,
            margin: const EdgeInsets.fromLTRB(4, 6, 4, 8),
            clipBehavior: Clip.antiAlias,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(DdRadius.md),
              gradient: const LinearGradient(colors: [DdColors.hostCardFrom, DdColors.hostCardTo]),
            ),
            child: Material(
              color: Colors.transparent,
              child: InkWell(
                onTap: () => ddHost.expanded = true,
                child: Stack(children: [
                  Positioned(
                    right: 8,
                    top: 22,
                    child: ddIcon(DdIcon.screenShare, size: 76, strokeWidth: 1.4, color: DdColors.onAccent.withAlpha(36)),
                  ),
                  Padding(
                    padding: const EdgeInsets.fromLTRB(16, 18, 16, 0),
                    child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                      Text(translate('This computer is being controlled'),
                          style: const TextStyle(fontSize: 16, fontWeight: FontWeight.w700, color: DdColors.onAccent)),
                      const SizedBox(height: 6),
                      Text(_countText(ddHost.count),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 12, color: DdColors.onAccent.withAlpha(230))),
                    ]),
                  ),
                  Positioned(
                    right: 10,
                    top: 10,
                    child: ddIcon(DdIcon.maximize2, size: 16, color: DdColors.onAccent),
                  ),
                ]),
              ),
            ),
          );
        },
      );
}

class _ClientRow extends StatelessWidget {
  final DdHostClient client;
  const _ClientRow({required this.client});

  String get _type {
    if (client.isFile) return translate('File transfer');
    if (client.isTerminal) return translate('Terminal');
    if (client.isPortForward) return translate('Port mapping');
    if (client.isCamera) return translate('View camera');
    return translate('Remote control');
  }

  /// 系统小方块：照首页的系统图标，认不出的系统用灰底
  Widget get _os {
    final p = client.platform.toLowerCase();
    final (icon, color) = p.contains('windows')
        ? (DdIcon.osWindows, DdColors.osWindows)
        : p.contains('linux')
            ? (DdIcon.osLinux, DdColors.osLinux)
            : p.contains('mac')
                ? (DdIcon.osMac, DdColors.osMac)
                : (DdIcon.monitor, DdColors.offline);
    return Container(
      width: 36,
      height: 36,
      alignment: Alignment.center,
      decoration: BoxDecoration(color: color, borderRadius: BorderRadius.circular(DdRadius.md)),
      child: ddIcon(icon, size: 20, color: DdColors.onAccent),
    );
  }

  @override
  Widget build(BuildContext context) {
    final device = client.device.isEmpty ? translate('Unknown device') : client.device.toUpperCase();
    return Container(
      height: 70,
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.symmetric(horizontal: 16),
      decoration: BoxDecoration(
        color: DdColors.surface,
        border: Border.all(color: DdColors.hostRowBorder),
        borderRadius: BorderRadius.circular(DdRadius.md),
      ),
      child: Row(children: [
        _os,
        const SizedBox(width: 14),
        Expanded(
          child: Column(mainAxisAlignment: MainAxisAlignment.center, crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text.rich(
              TextSpan(children: [
                TextSpan(text: device),
                if (client.account.isNotEmpty)
                  TextSpan(text: ' · ${client.account}', style: const TextStyle(color: DdColors.ink2)),
              ]),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(fontSize: 14, color: DdColors.ink),
            ),
            const SizedBox(height: 4),
            Row(children: [
              Container(
                height: 18,
                padding: const EdgeInsets.symmetric(horizontal: 4),
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  border: Border.all(color: DdColors.tagBorder),
                  borderRadius: BorderRadius.circular(3),
                ),
                child: Text(_type, style: const TextStyle(fontSize: 12, height: 1, color: DdColors.ink2)),
              ),
              const SizedBox(width: 6),
              Text('${translate('Connection time')} ${_clock(client.since == 0 ? 0 : _now() - client.since)}',
                  style: const TextStyle(fontSize: 12, color: DdColors.ink2, fontFeatures: [FontFeature.tabularFigures()])),
            ]),
          ]),
        ),
        _OutlineButton(text: translate('Disconnect remote control'), onTap: () => ddHost.disconnect(client.id)),
      ]),
    );
  }
}

/// 白底细边的小按钮（原型里「断开远控」「断开所有远控」：高 28、左右 12、4 圆角）
class _OutlineButton extends StatelessWidget {
  final String text;
  final VoidCallback onTap;
  const _OutlineButton({required this.text, required this.onTap});

  @override
  Widget build(BuildContext context) => DdHover(
        cursor: SystemMouseCursors.click,
        builder: (context, hover) => GestureDetector(
          onTap: onTap,
          child: Container(
            height: 28,
            padding: const EdgeInsets.symmetric(horizontal: 12),
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: hover ? DdColors.chrome : DdColors.surface,
              border: Border.all(color: DdColors.line),
              borderRadius: BorderRadius.circular(DdRadius.sm),
            ),
            child: Text(text, style: const TextStyle(fontSize: 13, color: DdColors.ink)),
          ),
        ),
      );
}

class _IconTap extends StatelessWidget {
  final DdIcon icon;
  final double size;
  final Color color;
  final VoidCallback onTap;
  const _IconTap({required this.icon, required this.size, required this.color, required this.onTap});

  @override
  Widget build(BuildContext context) => MouseRegion(
        cursor: SystemMouseCursors.click,
        child: GestureDetector(onTap: onTap, child: ddIcon(icon, size: size, color: color)),
      );
}
