// 受管版被控端连接管理小窗（有人连进来时右下角那个）的内容（用户 10-27 定方案 A：只换外观，不动布局和行为）。
// 原型：dreamdesk/design/prototype/Cm.html。server_page.dart 里只留几行钩子；点击节流（checkClickTime）、权限开关、
// 接受 / 断开 / 语音通话这些逻辑仍是它那边的，这里只管画。标题栏见 dd_sub_windows.dart（标签栏共用）。
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/server_model.dart';
import 'package:get/get.dart';

import 'dd_icons.dart';
import 'dd_theme.dart';

/// 页面两边的留白：加上 buildConnectionCard 自带的 8，共 18（原型 padding: 18）
const _kSide = EdgeInsets.symmetric(horizontal: 10);

/// 连进来的人：头像、名字、来源、连接时长、状态（原型 Cm.html 的白卡片）。[onChat] 不为空时右边多一个聊天 / 传输记录的切换按钮
class DdCmHeader extends StatelessWidget {
  final Client client;
  final RxInt time;
  final VoidCallback? onChat;
  const DdCmHeader({super.key, required this.client, required this.time, this.onChat});

  Widget _avatar() {
    final initial = Container(
      width: 40,
      height: 40,
      alignment: Alignment.center,
      decoration: const BoxDecoration(color: DdColors.accent, shape: BoxShape.circle),
      child: Text(client.name.isNotEmpty ? client.name.characters.first : '?',
          style: const TextStyle(fontSize: 16, color: DdColors.onAccent)),
    );
    return buildAvatarWidget(avatar: client.avatar, size: 40, borderRadius: 20, fallback: initial) ?? initial;
  }

  /// 第二行：对方的设备号，加上这次连接是干什么的（终端、传文件、看摄像头、端口映射）
  String get _sub {
    final extra = switch (client.type_()) {
      ClientType.terminal => translate('Terminal'),
      ClientType.file => translate('Transfer file'),
      ClientType.camera => translate('View camera'),
      _ => client.portForward.isNotEmpty ? 'Port Forward: ${client.portForward}' : '',
    };
    return extra.isEmpty ? client.peerId : '${client.peerId} · $extra';
  }

  @override
  Widget build(BuildContext context) {
    final connected = client.authorized && !client.disconnected;
    final status = client.authorized
        ? client.disconnected
            ? translate('Disconnected')
            : translate('Connected')
        : '${translate('Request access to your device')}...';
    final dot = connected ? DdColors.online : (client.authorized ? DdColors.offline : DdColors.busy);
    return Padding(
      padding: _kSide.copyWith(top: 10, bottom: 6),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        if (connected && client.type_() == ClientType.remote) ...[
          Text(translate('Someone is remotely using this computer'),
              style: const TextStyle(fontSize: 12, color: DdColors.ink2)),
          const SizedBox(height: 14),
        ],
        Container(
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
              color: DdColors.surface, borderRadius: BorderRadius.circular(DdRadius.lg), boxShadow: DdShadow.card),
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            Row(children: [
              _avatar(),
              const SizedBox(width: 12),
              Expanded(
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(client.name,
                      maxLines: 1, overflow: TextOverflow.ellipsis, style: const TextStyle(fontWeight: FontWeight.w600)),
                  Text(_sub,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: 12, color: DdColors.ink2)),
                ]),
              ),
              if (client.authorized)
                Obx(() => Text(formatDurationToTime(Duration(seconds: time.value)),
                    style: const TextStyle(fontSize: 12, color: DdColors.ink2, fontFeatures: [FontFeature.tabularFigures()]))),
              if (onChat != null)
                Tooltip(
                  message: translate(client.type_() == ClientType.file ? 'Transfer file' : 'Chat'),
                  // 点击区 32×32（原来只有图标那么大）
                  child: InkWell(
                    onTap: onChat,
                    borderRadius: BorderRadius.circular(DdRadius.md),
                    child: SizedBox(width: 32, height: 32, child: Center(child: ddIcon(DdIcon.messageSquare, size: 18))),
                  ),
                ),
            ]),
            const SizedBox(height: 12),
            Row(children: [
              Container(width: 6, height: 6, decoration: BoxDecoration(color: dot, shape: BoxShape.circle)),
              const SizedBox(width: 6),
              Flexible(
                  child: Text(status,
                      overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 12, color: DdColors.ink2))),
            ]),
          ]),
        ),
      ]),
    );
  }
}

/// 「允许对方」：一排小按钮，开着的是浅蓝底蓝字（原型 dd-cm）。开关的动作由 [onToggle] 去做（含点击节流），
/// 做完调 [apply]（它改本地记着的权限、刷新画面）——和原来一样，节流没放行就一起不做
class DdCmPermissions extends StatefulWidget {
  final Client client;
  final bool canModify;
  final void Function(String name, bool enabled, VoidCallback apply) onToggle;
  const DdCmPermissions({super.key, required this.client, required this.canModify, required this.onToggle});

  @override
  State<DdCmPermissions> createState() => _DdCmPermissionsState();
}

class _DdCmPermissionsState extends State<DdCmPermissions> {
  Client get c => widget.client;

  /// (给 cmSwitchPermission 的名字, 按钮上的字, 悬停说明, 现在开没开, 改本地值)。
  /// 这份清单照抄 server_page.dart 的 `_PrivilegeBoard`（受管版换了排法，权限项一样）：合并上游时，上游在那里增删权限，这里要同步
  List<(String, String, String, bool, void Function(bool))> get _items => c.type_() == ClientType.camera
      ? [
          ('audio', 'Sound', 'Enable audio', c.audio, (v) => c.audio = v),
          ('recording', 'Screen recording', 'Enable recording session', c.recording, (v) => c.recording = v),
        ]
      : [
          ('keyboard', 'Keyboard & mouse', 'Enable keyboard/mouse', c.keyboard, (v) => c.keyboard = v),
          ('clipboard', 'Clipboard', 'Enable clipboard', c.clipboard, (v) => c.clipboard = v),
          ('audio', 'Sound', 'Enable audio', c.audio, (v) => c.audio = v),
          ('file', 'Files', 'Enable file copy and paste', c.file, (v) => c.file = v),
          ('restart', 'Restart', 'Enable remote restart', c.restart, (v) => c.restart = v),
          ('recording', 'Screen recording', 'Enable recording session', c.recording, (v) => c.recording = v),
          // 只有 Windows 能屏蔽输入
          if (isWindows)
            ('block_input', 'Block input', 'Enable blocking user input', c.blockInput, (v) => c.blockInput = v),
          if (bind.mainSupportedPrivacyModeImpls() != '[]')
            ('privacy_mode', 'Privacy mode', 'Enable privacy mode', c.privacyMode, (v) => c.privacyMode = v),
        ];

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: _kSide.copyWith(top: 8, bottom: 6),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Text(translate('Allow the other side'), style: const TextStyle(fontSize: 12, color: DdColors.ink2)),
        const SizedBox(height: 8),
        LayoutBuilder(builder: (context, box) {
          final items = _items;
          final labels = [for (final it in items) translate(it.$2)];
          // 一排四个（原型）；这个语言的字放不下就改一排三个、两个，不截断。中文都是三四个字，一排四个放得下；英文「Keyboard & mouse」要两个一排
          final cols = _columns(box.maxWidth, labels);
          final w = (box.maxWidth - (cols - 1) * 8) / cols;
          return Wrap(spacing: 8, runSpacing: 8, children: [
            for (var i = 0; i < items.length; i++)
              SizedBox(
                width: w,
                child: Tooltip(
                  message: '${translate(items[i].$3)}: ${items[i].$4 ? 'ON' : 'OFF'}',
                  waitDuration: Duration.zero,
                  child: _Chip(
                    label: labels[i],
                    on: items[i].$4,
                    dim: items[i].$4 && !widget.canModify,
                    onTap: widget.canModify
                        ? () => widget.onToggle(items[i].$1, !items[i].$4, () => setState(() => items[i].$5(!items[i].$4)))
                        : null,
                  ),
                ),
              ),
          ]);
        }),
      ]),
    );
  }
}

const _kChipStyle = TextStyle(fontSize: 12);

/// 一排放几个按钮：从四个往下试，最长的字（加两边各 4 的内边距、边框 2）放得下就用
int _columns(double maxWidth, List<String> labels) {
  var widest = 0.0;
  for (final s in labels) {
    final p = TextPainter(text: TextSpan(text: s, style: _kChipStyle), textDirection: TextDirection.ltr, maxLines: 1)
      ..layout();
    if (p.width > widest) widest = p.width;
    p.dispose();
  }
  for (final cols in [4, 3]) {
    if ((maxWidth - (cols - 1) * 8) / cols >= widest + 10) return cols;
  }
  return 2;
}

class _Chip extends StatelessWidget {
  final String label;
  final bool on;
  final bool dim;
  final VoidCallback? onTap;
  const _Chip({required this.label, required this.on, required this.dim, this.onTap});

  @override
  Widget build(BuildContext context) {
    return Opacity(
      opacity: dim ? 0.6 : 1,
      child: MouseRegion(
        cursor: onTap == null ? SystemMouseCursors.basic : SystemMouseCursors.click,
        child: GestureDetector(
          onTap: onTap,
          child: Container(
            height: 32,
            alignment: Alignment.center,
            padding: const EdgeInsets.symmetric(horizontal: 4),
            decoration: BoxDecoration(
              color: on ? DdColors.accentSoft : DdColors.surface,
              border: Border.all(color: on ? DdColors.accentBorder : DdColors.line),
              borderRadius: BorderRadius.circular(DdRadius.sm),
            ),
            child: Text(label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: _kChipStyle.copyWith(color: on ? DdColors.accent : DdColors.ink3)),
          ),
        ),
      ),
    );
  }
}

/// 下面的按钮（原型：通栏）。原来每个按钮自己带颜色，换样子后按颜色归类：透明 = 次要（线框）；红 = 危险（白底红字）；其余 = 主按钮。
/// 靠 server_page.dart 的 `buildButton` 传来的颜色字面量认角色：合并上游时，上游改了那几个按钮的颜色，这里要同步。
/// [onClick] / [onTapDown] 已经是套过点击节流的
Widget ddCmButton({
  required Color? color,
  required String text,
  VoidCallback? onClick,
  GestureTapDownCallback? onTapDown,
  String? tooltip,
}) {
  final label = Text(translate(text));
  final VoidCallback onPressed = onClick ?? () {};
  final isRed = color == Colors.red || color == Colors.redAccent;
  Widget btn = SizedBox(
    width: double.infinity,
    child: color == Colors.transparent
        ? OutlinedButton(onPressed: onPressed, child: label)
        : isRed
            ? OutlinedButton(
                onPressed: onPressed,
                style: OutlinedButton.styleFrom(foregroundColor: DdColors.dangerText),
                child: label)
            : ElevatedButton(onPressed: onPressed, child: label),
  );
  // 菜单要在按下的位置弹出（音频输入），所以另外听按下
  if (onTapDown != null) {
    // 只认左键（原来的 InkWell.onTapDown 也只认主键）
    btn = Listener(
        onPointerDown: (e) {
          if (e.buttons == kPrimaryButton) onTapDown(TapDownDetails(globalPosition: e.position));
        },
        child: btn);
  }
  if (tooltip != null) btn = Tooltip(message: translate(tooltip), child: btn);
  return Padding(padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4), child: btn);
}
