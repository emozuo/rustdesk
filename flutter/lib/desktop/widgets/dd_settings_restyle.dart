// 受管版下 RustDesk 设置页（desktop_setting_page.dart）的样子（用户 10-27 定方案 A：只换外观，不动布局和行为）。
// 设置页那边只留 `if (kDdManaged) return ...` 一行钩子，逻辑（读写选项、锁定判断、切换分类、解锁）照用它的，这里只管画。
// 原型：dreamdesk/design/prototype/Set*.html（样式变量 dd.css）。从 dd_restyle.dart 分出来：common.dart 引用 dd_restyle，
// dd_restyle 再引用设置页就绕成了 common → dd_restyle → 设置页 → common 的圈
import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/widgets/dialog.dart';
import 'package:flutter_hbb/desktop/pages/desktop_setting_page.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:get/get.dart';

import 'dd_controls.dart';
import 'dd_icons.dart';
import 'dd_theme.dart';

/// 设置页左上的「设置」：黑色粗体（RustDesk 是蓝色细体）
Widget ddSettingsHeader(String text) {
  return SizedBox(
    height: 62,
    child: Align(
      alignment: Alignment.centerLeft,
      child: Text(text, style: const TextStyle(fontSize: 20, fontWeight: FontWeight.w600, color: DdColors.ink)),
    ),
  ).marginOnly(left: 20, top: 10);
}

/// 设置页左边分类的图标（原型 SetTabsA.html，Lucide；常规不用齿轮，左栏底部「设置」已经是齿轮）
const _settingsTabIcons = {
  SettingsTabKey.general: DdIcon.sliders,
  SettingsTabKey.safety: DdIcon.shield,
  SettingsTabKey.network: DdIcon.network,
  SettingsTabKey.display: DdIcon.monitor,
  SettingsTabKey.printer: DdIcon.printer,
  SettingsTabKey.about: DdIcon.info,
};

/// 设置页左边的分类（照首页左栏的行：圆角底色、选中时底色里有一道主色竖条；图标不随选中变色）。
/// 切换逻辑照抄 RustDesk 的 _listItem（desktop_setting_page.dart；合并上游时对照，见 dreamdesk/docs/合并上游检查清单.md）
Widget ddSettingsTabItem(
    {required String label,
    required SettingsTabKey tabKey,
    required Rx<SettingsTabKey> selectedTab,
    required PageController controller}) {
  return Obx(() {
    final selected = tabKey == selectedTab.value;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
      child: Material(
        color: selected ? DdColors.selected : Colors.transparent,
        borderRadius: BorderRadius.circular(DdRadius.md),
        child: InkWell(
          borderRadius: BorderRadius.circular(DdRadius.md),
          onTap: () {
            if (selectedTab.value != tabKey) {
              final index = DesktopSettingPage.tabKeys.indexOf(tabKey);
              if (index == -1) return;
              controller.jumpToPage(index);
            }
            selectedTab.value = tabKey;
          },
          child: SizedBox(
            height: DdSize.rowHeight,
            child: Row(children: [
              Container(
                width: 4,
                height: 16,
                margin: const EdgeInsets.only(left: 4, right: 10),
                decoration: BoxDecoration(
                    color: selected ? DdColors.accent : Colors.transparent, borderRadius: BorderRadius.circular(2)),
              ),
              ddIcon(_settingsTabIcons[tabKey] ?? DdIcon.settings, size: 18, color: DdColors.ink),
              const SizedBox(width: 12),
              Text(label, style: const TextStyle(fontSize: 14, color: DdColors.ink)),
            ]),
          ),
        ),
      ),
    );
  });
}

/// _Card 的受管版（原型 dd-group）：白底 8 圆角、不要阴影，组标题 13 号粗灰字。子项的边距照 RustDesk 的。
/// 底用 Material（不是普通的 Container）：卡片里 RustDesk 的 ListTile、InkWell 的悬停底色画在最近的 Material 上，
/// 用 Container 的话画到白底下面、看不见（RustDesk 原来的 Card 本身就是一层 Material）
Widget ddSettingsCard(
    {required String title,
    required List<Widget> children,
    List<Widget>? titleSuffix,
    required double width,
    required double leftMargin,
    required double hMargin}) {
  return Row(children: [
    Flexible(
      child: Container(
        width: width,
        margin: EdgeInsets.only(left: leftMargin, top: 16),
        child: Material(
          color: DdColors.surface,
          borderRadius: BorderRadius.circular(DdRadius.lg),
          child: Padding(
            padding: const EdgeInsets.only(bottom: 12),
            child: Column(children: [
              Row(children: [
                Expanded(
                    child: Text(title,
                        style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600, color: DdColors.ink2))),
                ...?titleSuffix,
              ]).marginOnly(left: hMargin, right: hMargin, top: 14, bottom: 6),
              ...children.map((e) => e.marginOnly(top: 4, right: hMargin)),
            ]),
          ),
        ),
      ),
    ),
  ]);
}

/// _OptionCheckBox 的受管版（原型 dd-sr）：文字在左、开关在右，整行可点。[value] 和 [onChanged] 就是 RustDesk 那份
/// （读写选项、锁定判断都在它那里）；[onChanged] 为空表示不能改
Widget ddOptionRow(
    {required String label,
    required RxBool value,
    required ValueChanged<bool>? onChanged,
    required bool enabled,
    Widget? trailing}) {
  return GestureDetector(
    // 整行（含上下空白、文字和开关之间的空档）都能点：默认只认画了东西的地方
    behavior: HitTestBehavior.opaque,
    onTap: onChanged == null ? null : () => onChanged(!value.value),
    child: Obx(() => ConstrainedBox(
          constraints: const BoxConstraints(minHeight: 36),
          child: Row(children: [
            Flexible(
              fit: trailing == null ? FlexFit.tight : FlexFit.loose,
              child: Text(label, style: TextStyle(fontSize: 14, color: enabled ? DdColors.ink : DdColors.ink3)),
            ),
            if (trailing != null) ...[trailing, const Spacer()],
            DdSwitch(value: value.value, onChanged: onChanged),
          ]),
        )),
  );
}

/// _Button / _SubButton 的受管版：白底细边的紧凑按钮（原型里设置页的按钮都是 dd-btn--secondary dd-btn--compact）
/// [style]：RustDesk 给个别按钮的自定义样式（Wayland 那两个红色的清除按钮），有就沿用它的实心按钮（它给了的优先，同 ddDialogButton）
Widget ddSettingsButton(String label, VoidCallback? onPressed, {String? tip, ButtonStyle? style}) {
  Widget b = style == null
      ? OutlinedButton(onPressed: onPressed, style: ddCompactButton, child: Text(label))
      : ElevatedButton(onPressed: onPressed, style: ddCompactWith(style), child: Text(label));
  if (tip != null) b = Tooltip(message: tip, child: b);
  return Row(children: [b]);
}

/// 按钮已经写了「解锁」，提示条上换成说明为什么要解锁（原型 SetSecurity / SetNetwork）
const _unlockTips = {
  'Unlock Security Settings': 'Unlock to change security settings (administrator approval required)',
  'Unlock Network Settings': 'Unlock to change network settings (administrator approval required)',
};

/// _lock 的受管版（原型 dd-tip）：浅蓝提示条，盾牌图标 + 说明 + 「解锁」主按钮。解锁走的路和 RustDesk 的一样。
/// [label] 是 RustDesk 传的原文（没翻译）
Widget ddUnlockTip(String label, VoidCallback onUnlock, {required double width, required double leftMargin}) {
  return Row(children: [
    Flexible(
      child: Container(
        width: width,
        margin: EdgeInsets.only(left: leftMargin, top: 16),
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
        decoration:
            BoxDecoration(color: DdColors.accentSoft, borderRadius: BorderRadius.circular(DdRadius.md)),
        child: Row(children: [
          ddIcon(DdIcon.shield, size: 18, color: DdColors.accent),
          const SizedBox(width: 8),
          Expanded(
              child: Text(translate(_unlockTips[label] ?? label),
                  style: const TextStyle(fontSize: 13, color: DdColors.ink))),
          ElevatedButton(
              onPressed: () => _unlock(onUnlock), style: ddCompactButton, child: Text(translate('Unlock'))),
        ]),
      ),
    ),
  ]);
}

/// 解锁：照抄 RustDesk 设置页 _lock 里那个按钮的 onPressed（desktop_setting_page.dart）。安全相关——上游改了解锁规则这里要跟，
/// 见 dreamdesk/docs/合并上游检查清单.md
Future<void> _unlock(VoidCallback onUnlock) async {
  final unlockPin = bind.mainGetUnlockPin();
  if (unlockPin.isEmpty || isUnlockPinDisabled()) {
    if (await callMainCheckSuperUserPermission()) onUnlock();
  } else {
    checkUnlockPinDialog(unlockPin, onUnlock);
  }
}
