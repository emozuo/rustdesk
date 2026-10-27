// 受管版下 RustDesk 原有的弹窗、下拉框的样子（用户 10-27 定方案 A：只换外观，不动布局和行为）。
// common.dart 那边只留 `if (kDdManaged) return ...` 一行钩子，逻辑（关弹窗、选中项）照用它的，这里只管画。
// 原型：dreamdesk/design/prototype/Dialogs.html（样式变量 dd.css）。设置页的在 dd_settings_restyle.dart，
// 我们自己页面的通用控件在 dd_controls.dart（这里转导出，原来从这里拿控件的文件不用改引用）
import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:get/get.dart';

import 'dd_controls.dart';
import 'dd_icons.dart';
import 'dd_theme.dart';

export 'dd_controls.dart';

/// dialogButton 的受管版：同样的主按钮 / 线框按钮，换成紧凑尺寸。RustDesk 给按钮带的图标（关闭、对勾等）不画——
/// 原型里弹窗按钮都只有文字
Widget ddDialogButton(String text,
    {required VoidCallback? onPressed, bool isOutline = false, TextStyle? style, ButtonStyle? buttonStyle}) {
  final label = Text(translate(text), style: style);
  // 线框按钮照 RustDesk 不用调用方的 buttonStyle（「Connect via relay」给线框按钮传了绿底，原版也不画）
  if (isOutline) return OutlinedButton(onPressed: onPressed, style: ddCompactButton, child: label);
  // 调用方给的（红色删除、绿色等）优先，紧凑尺寸补空
  return ElevatedButton(onPressed: onPressed, style: ddCompactWith(buttonStyle), child: label);
}

final _link = RegExp(r'(https?://[^\s]+)');

/// msgboxContent 的受管版（原型 dd-modal）：22 号图标和 18 号粗标题一行，下面 14 号灰色正文。
/// [title] 是没翻译的原文（按它认出「连接授权没通过」），[text] 是已经翻译好的正文
Widget ddMsgboxContent(String type, String title, String text) {
  final icon = _msgboxIcon(type, title);
  return Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    mainAxisSize: MainAxisSize.min,
    children: [
      Row(children: [
        if (icon != null) ...[icon, const SizedBox(width: 10)],
        Expanded(
          child: Text(translate(title),
              style: const TextStyle(fontSize: 18, height: 26 / 18, fontWeight: FontWeight.w600, color: DdColors.ink)),
        ),
      ]),
      const SizedBox(height: 12),
      DefaultTextStyle.merge(
        style: const TextStyle(fontSize: 14, height: 22 / 14, color: DdColors.ink2),
        // 带链接的沿用 RustDesk 的（链接可点）；它写死 15 号字，差一号不另抄一份
        child: _link.hasMatch(text) ? createDialogContent(text) : SelectableText(text),
      ),
    ],
  ).marginOnly(bottom: 8);
}

Widget? _msgboxIcon(String type, String title) {
  Widget icon(DdIcon i, Color c) => ddIcon(i, size: 22, color: c);
  if (type == 'connecting') {
    return const SizedBox(width: 22, height: 22, child: CircularProgressIndicator(strokeWidth: 2));
  }
  // 受管版要不到连接票（src/ddpro/managed_client.rs 的标题）
  if (title == 'Connection authorization failed') return icon(DdIcon.shieldAlert, DdColors.dangerText);
  // 和 RustDesk 的 msgboxIcon 同样的优先级：它按 错误、成功、等待、提权、信息 依次覆盖，后面的算数，这里倒过来先判
  if (type.contains('info')) return icon(DdIcon.info, DdColors.accent);
  if (type == 'on-uac' || type == 'on-foreground-elevated') return icon(DdIcon.shieldCheck, DdColors.accent);
  if (type == 'wait-uac' || type == 'wait-remote-accept-nook') return icon(DdIcon.hourglass, DdColors.accent);
  if (type.contains('success')) return icon(DdIcon.circleCheck, DdColors.success);
  if (type.contains('error') || type == 're-input-password') return icon(DdIcon.circleX, DdColors.dangerText);
  return null;
}

/// common.dart 的 ComboBox 的受管版（原型 dd-select）：高 32、细边、4 圆角、13 号字、灰色下箭头。
/// 选中逻辑照抄它的（合并上游时对照，见 dreamdesk/docs/合并上游检查清单.md）
Widget ddComboBox(ComboBox w) {
  final index = w.keys.indexOf(w.initialKey).clamp(0, w.keys.length - 1);
  final ref = w.values[index].obs;
  w.current = w.keys[index];
  return Container(
    height: 32,
    decoration: BoxDecoration(
      color: DdColors.surface,
      border: Border.all(color: DdColors.line),
      borderRadius: BorderRadius.circular(DdRadius.sm),
    ),
    child: Obx(() => DropdownButton<String>(
          isExpanded: true,
          // 左右留白放进下拉框自己里面：放在外面的话，框的左 10、右 8 像素点不中
          padding: const EdgeInsets.only(left: 10, right: 8),
          value: ref.value,
          underline: const SizedBox.shrink(),
          focusColor: Colors.transparent,
          borderRadius: BorderRadius.circular(DdRadius.lg),
          style: TextStyle(fontSize: 13, color: w.enabled ? DdColors.ink : DdColors.ink3),
          icon: ddIcon(DdIcon.chevronDown, size: 16, color: DdColors.ink2),
          onChanged: w.enabled
              ? (v) {
                  if (v != null && v != ref.value) {
                    ref.value = v;
                    w.current = v;
                    w.onChanged(w.keys[w.values.indexOf(v)]);
                  }
                }
              : null,
          items: w.values
              .map((v) => DropdownMenuItem<String>(value: v, child: Text(v, overflow: TextOverflow.ellipsis)))
              .toList(),
        )),
  ).marginOnly(bottom: 5);
}
