// DreamDesk：对端支持专业模式、但这次会话现在不在专业模式时，远程画面左上角的常驻标签（「不静默降级」），
// 以及专业会话期间通用窗口的隐藏 / 显示 / 关闭（P1 第 2 项「专业会话只有一个窗口」）。
//
// Rust 侧 `src/ddpro/view_link.rs` 推两种事件：
// - `ddpro_mode`：`level` 为 `fault`（红，故障或条件不满足），空串表示撤掉；`text`（结论）和 `tip`（意味着什么、怎么处理）
//   是多语言表的键，`detail` 是原始错误信息。鼠标停上去先看说明，原始错误跟在后面（和日志一字不差，排查时拿它去搜）。
//   标签上带「重新进入专业模式」按钮，值不值得再试由 Rust 侧判断（不值得时点了不起作用）。
// - `ddpro_window`：`action` 为 `hide`（专业画面出来了，屏幕上只留它）、`show`（专业模式出故障或连接断了，回到这个窗口）、
//   `close`（用户关了专业画面，结束这次远程）。
// 不带专业模式的构建收不到这两个事件，标签不出现、窗口照常。
import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hbb/main.dart';
import 'package:get/get.dart';

import '../../common.dart';
import '../../models/platform_model.dart';

class _Notice {
  final bool fault;
  final String text;
  final String tip;
  final String detail;
  const _Notice(this.fault, this.text, this.tip, this.detail);
}

final _notices = <SessionID, Rx<_Notice?>>{};

Rx<_Notice?> _notice(SessionID id) =>
    _notices.putIfAbsent(id, () => Rx<_Notice?>(null));

void onDdproModeEvent(SessionID sessionId, Map<String, dynamic> evt) {
  final level = evt['level'] as String? ?? '';
  _notice(sessionId).value = level.isEmpty
      ? null
      : _Notice(level == 'fault', evt['text'] as String? ?? '',
          evt['tip'] as String? ?? '', evt['detail'] as String? ?? '');
}

void onDdproWindowEvent(
    SessionID sessionId, String peerId, Map<String, dynamic> evt) async {
  final id = kWindowId;
  if (id == null) return;
  switch (evt['action'] as String? ?? '') {
    case 'hide':
      WindowController.fromWindowId(id).hide();
      break;
    case 'show':
      windowOnTop(id);
      break;
    case 'close':
      // 这个窗口是隐藏着的：Flutter 不再出帧，移除标签后远程页面的销毁（RustDesk 在那里关会话）不会发生，
      // 关窗的循环也因为「已经隐藏」而不动（2026-10-20 瘦客户机实撞：断开后连接还在）。
      // 所以先直接关会话（页面销毁时调的同一个），再移除标签、按 RustDesk 子窗口的关闭流程关窗；
      // 之后页面真销毁时再关一次，会话已经不在了，什么也不做
      await bind.sessionClose(sessionId: sessionId);
      closeConnection(id: peerId);
      await WindowController.fromWindowId(id).close();
      break;
  }
}

class DdproModeBadge extends StatelessWidget {
  final SessionID sessionId;
  const DdproModeBadge({Key? key, required this.sessionId}) : super(key: key);

  @override
  Widget build(BuildContext context) => Obx(() {
        final n = _notice(sessionId).value;
        if (n == null) return const Offstage();
        final text = Text(translate(n.text),
            style: const TextStyle(color: Colors.white, fontSize: 13));
        final hint = [
          if (n.tip.isNotEmpty) translate(n.tip),
          if (n.detail.isNotEmpty)
            '${translate('Original error (for troubleshooting)')}: ${n.detail}',
        ].join('\n\n');
        return Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          decoration: BoxDecoration(
            color: (n.fault ? Colors.red.shade700 : Colors.grey.shade700)
                .withAlpha(220),
            borderRadius: BorderRadius.circular(4),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              hint.isEmpty ? text : Tooltip(message: hint, child: text),
              if (n.fault) ...[
                const SizedBox(width: 8),
                InkWell(
                  onTap: () => bind.sessionDdproRetry(sessionId: sessionId),
                  child: Text(translate('Re-enter pro mode'),
                      style: const TextStyle(
                          color: Colors.white,
                          fontSize: 13,
                          decoration: TextDecoration.underline)),
                ),
              ],
            ],
          ),
        );
      });
}
