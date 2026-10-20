// DreamDesk：专业窗口开着时，RustDesk 质量面板末尾多几行——专业画面的延迟、收到的码率、位深、丢包与补发、
// 显示链路检查、连接是否加密。
//
// Rust 侧 `src/ddpro/view_link.rs` 每秒推 `ddpro_quality` 事件；不带数据的事件表示专业窗口关了，撤掉这几行。
// 帧率、编码、色度 RustDesk 面板本来就有，Rust 侧直接把它们换成专业画面的实测值，不在这里。
// 面板原有的标签不翻译，这里的也保持英文；显示链路问题的说明（鼠标停在那一行上显示）是多语言表的键，要翻译。
// 不带专业模式的构建收不到这个事件，这几行不出现。
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:get/get.dart';

import '../../common.dart';
import '../../models/model.dart';

final _stats = <SessionID, Rx<Map<String, dynamic>?>>{};

Rx<Map<String, dynamic>?> _of(SessionID id) =>
    _stats.putIfAbsent(id, () => Rx<Map<String, dynamic>?>(null));

void onDdproQualityEvent(SessionID sessionId, Map<String, dynamic> evt) {
  _of(sessionId).value = evt.containsKey('kbps') ? Map.of(evt) : null;
}

typedef QualityRow = Widget Function(String info, String? value,
    {Color? rightColor});

/// 质量面板末尾的几行，`row` 用面板自己的行样式。
Widget ddproQualityRows(FFI? ffi, QualityRow row) {
  if (ffi == null) return const SizedBox.shrink();
  final id = ffi.ffiModel.sessionId;
  return Obx(() {
    final v = _of(id).value;
    if (v == null) return const SizedBox.shrink();
    final latency = v['latency_ms']?.toString() ?? '-1';
    final secure = ffi.ffiModel.secure;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        row('Pro latency', latency == '-1' ? '-' : '${latency}ms',
            rightColor: Colors.green),
        row('Received', '${v['kbps'] ?? '-'}kb'),
        row('Bit depth', '${v['bit_depth'] ?? '-'}bit'),
        row('Lost / recovered', '${v['lost'] ?? '-'} / ${v['recovered'] ?? '-'}'),
        _displayRow(v, row),
        row(
            'Encryption',
            secure == null
                ? '-'
                : secure
                    ? 'Encrypted'
                    : 'Not encrypted',
            rightColor: secure == false ? Colors.red : null),
      ],
    );
  });
}

/// 显示链路问题的代号（Rust 侧 `src/ddpro/view.rs`）→ 面板里的短名、处理办法（多语言表的键，鼠标停上去显示）
const _issues = <String, (String, String)>{
  'limited_range': (
    'Limited range',
    'The display connection sends limited-range RGB (16-235), so colors look washed out unless the monitor expects limited range too. A resolution that is not a TV format avoids it.'
  ),
  'night_light': (
    'Night light',
    'Night light is on and tints the whole screen. Turn it off in Settings > Displays.'
  ),
  'gamma_curve': (
    'Gamma curve',
    "The display's gamma curve is altered, for example by a calibrated color profile. Remove it in Settings > Color."
  ),
  'color_transform': (
    'Color matrix',
    'The display applies a color matrix or curve to the output.'
  ),
  'scaled': (
    'Scaled',
    "The pro view is scaled, so remote pixels are not shown one to one. Set the display scale to 100% and match the remote screen's resolution."
  ),
};

/// 显示链路：没问题一行绿色「OK」；有问题每项一行红色短名（面板上直接看得到），处理办法在鼠标悬停提示里；
/// 读不了显示属性时「Unknown」。
Widget _displayRow(Map<String, dynamic> v, QualityRow row) {
  if (v['display_checked']?.toString() != 'true') {
    return row('Display output', 'Unknown');
  }
  var issues = <String>[];
  try {
    issues = (jsonDecode(v['display_issues']?.toString() ?? '[]') as List)
        .map((e) => e.toString())
        .toList();
  } catch (_) {}
  if (issues.isEmpty) {
    return row('Display output', 'OK', rightColor: Colors.green);
  }
  return Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: issues.map((code) {
      final (name, tip) = _issues[code] ?? (code, '');
      final r = row('Display output', name, rightColor: Colors.red);
      return tip.isEmpty ? r : Tooltip(message: translate(tip), child: r);
    }).toList(),
  );
}
