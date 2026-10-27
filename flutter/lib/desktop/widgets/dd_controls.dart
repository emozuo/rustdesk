// 受管版自己页面通用的小部件（首页、端口映射、文件传输、连接管理器等都用）：紧凑按钮、小开关、输入框、状态圆点、头像、卡片底。
// 和 dd_restyle.dart 分开：那里是 RustDesk 原有页面（弹窗、下拉框）换外观用的，这里是我们自己画界面用的
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hbb/common.dart';

import 'dd_theme.dart';

/// 紧凑按钮（原型 dd-btn--compact）：高 30、左右 14、13 号字。颜色由主题的 Elevated / Outlined 按钮给
const ddCompactButton = ButtonStyle(
  minimumSize: WidgetStatePropertyAll(Size(0, 30)),
  padding: WidgetStatePropertyAll(EdgeInsets.symmetric(horizontal: 14)),
  textStyle: WidgetStatePropertyAll(TextStyle(fontSize: 13)),
);

/// 紧凑按钮加调用方自己的样式（红色删除、绿色等）：调用方给了的优先，没给的用紧凑尺寸补
ButtonStyle ddCompactWith(ButtonStyle? style) => style?.merge(ddCompactButton) ?? ddCompactButton;

/// 白底卡片（原型 dd-card）：8 圆角、卡片阴影
BoxDecoration ddCardDecoration({Color color = DdColors.surface}) =>
    BoxDecoration(color: color, borderRadius: BorderRadius.circular(DdRadius.lg), boxShadow: DdShadow.card);

/// 状态小圆点（在线绿、在用橙等）
class DdDot extends StatelessWidget {
  final Color color;
  final double size;
  const DdDot({super.key, required this.color, this.size = 6});

  @override
  Widget build(BuildContext context) =>
      Container(width: size, height: size, decoration: BoxDecoration(color: color, shape: BoxShape.circle));
}

/// 登录用户的头像：主色圆、白字写名字的首字
class DdAvatar extends StatelessWidget {
  final String name;
  final double size;
  final double fontSize;
  const DdAvatar({super.key, required this.name, required this.size, required this.fontSize});

  @override
  Widget build(BuildContext context) => Container(
        width: size,
        height: size,
        alignment: Alignment.center,
        decoration: const BoxDecoration(color: DdColors.accent, shape: BoxShape.circle),
        child: Text(name.isEmpty ? '' : name.characters.first.toUpperCase(),
            style: TextStyle(fontSize: fontSize, color: DdColors.onAccent)),
      );
}

/// 小开关（原型 dd-switch）：36×20，开是主色、关是灰；[onChanged] 为空时半透明、点不动
class DdSwitch extends StatelessWidget {
  final bool value;
  final ValueChanged<bool>? onChanged;
  const DdSwitch({super.key, required this.value, this.onChanged});

  @override
  Widget build(BuildContext context) {
    final on = onChanged != null;
    return MouseRegion(
      cursor: on ? SystemMouseCursors.click : SystemMouseCursors.basic,
      child: GestureDetector(
        onTap: on ? () => onChanged!(!value) : null,
        child: Opacity(
          opacity: on ? 1 : 0.5,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 150),
            width: 36,
            height: 20,
            padding: const EdgeInsets.all(2),
            alignment: value ? Alignment.centerRight : Alignment.centerLeft,
            decoration: BoxDecoration(
                color: value ? DdColors.accent : DdColors.disabledFill, borderRadius: BorderRadius.circular(10)),
            child: Container(
                width: 16, height: 16, decoration: const BoxDecoration(color: DdColors.surface, shape: BoxShape.circle)),
          ),
        ),
      ),
    );
  }
}

/// 输入框（原型 dd-input）：高 32、4 圆角、聚焦时主色边加一圈浅蓝。紧凑的内边距只在这里设——
/// 主题的 inputDecorationTheme 管着 RustDesk 原有页面里所有的输入框，不动它
class DdInput extends StatefulWidget {
  final TextEditingController controller;
  final List<TextInputFormatter>? inputFormatters;
  final String? hint;

  /// 读屏用的名字
  final String? label;
  const DdInput({super.key, required this.controller, this.inputFormatters, this.hint, this.label});

  @override
  State<DdInput> createState() => _DdInputState();
}

class _DdInputState extends State<DdInput> {
  final _focus = FocusNode();
  bool _focused = false;

  @override
  void initState() {
    super.initState();
    _focus.addListener(() => setState(() => _focused = _focus.hasFocus));
  }

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 32,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(DdRadius.sm),
        boxShadow: _focused ? const [BoxShadow(color: DdColors.accentSoft, spreadRadius: 3)] : null,
      ),
      child: Semantics(
        label: widget.label,
        child: TextField(
          controller: widget.controller,
          focusNode: _focus,
          inputFormatters: widget.inputFormatters,
          textAlignVertical: TextAlignVertical.center,
          style: const TextStyle(fontSize: 14),
          decoration: InputDecoration(
            isDense: true,
            hintText: widget.hint,
            contentPadding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
          ),
        ).workaroundFreezeLinuxMint(),
      ),
    );
  }
}
