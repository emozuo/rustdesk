// DreamDesk：对端支持专业模式、但这次会话现在不在专业模式时，远程画面左上角的常驻标签（「不静默降级」）。
//
// Rust 侧 `src/ddpro/view_link.rs` 推 `ddpro_mode` 事件：`level` 为 `fault`（红，故障或条件不满足）、
// `user`（灰，用户自己关了专业窗口），空串表示撤掉；`text` 是多语言表的键，`detail` 是原始错误信息（鼠标停上去看）。
// 不带专业模式的构建收不到这个事件，标签不出现。
import 'package:flutter/material.dart';
import 'package:get/get.dart';

import '../../common.dart';

class _Notice {
  final bool fault;
  final String text;
  final String detail;
  const _Notice(this.fault, this.text, this.detail);
}

final _notices = <SessionID, Rx<_Notice?>>{};

Rx<_Notice?> _notice(SessionID id) =>
    _notices.putIfAbsent(id, () => Rx<_Notice?>(null));

void onDdproModeEvent(SessionID sessionId, Map<String, dynamic> evt) {
  final level = evt['level'] as String? ?? '';
  _notice(sessionId).value = level.isEmpty
      ? null
      : _Notice(level == 'fault', evt['text'] as String? ?? '',
          evt['detail'] as String? ?? '');
}

class DdproModeBadge extends StatelessWidget {
  final SessionID sessionId;
  const DdproModeBadge({Key? key, required this.sessionId}) : super(key: key);

  @override
  Widget build(BuildContext context) => Obx(() {
        final n = _notice(sessionId).value;
        if (n == null) return const Offstage();
        final label = Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          decoration: BoxDecoration(
            color: (n.fault ? Colors.red.shade700 : Colors.grey.shade700)
                .withAlpha(220),
            borderRadius: BorderRadius.circular(4),
          ),
          child: Text(translate(n.text),
              style: const TextStyle(color: Colors.white, fontSize: 13)),
        );
        return n.detail.isEmpty
            ? label
            : Tooltip(message: n.detail, child: label);
      });
}
