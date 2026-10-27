// 受管版文件传输窗口内容区的样子（用户 10-27 定方案 A：只换外观，不动布局和行为）。
// file_manager_page.dart 里每处只包一层钩子：把 RustDesk 原来那个部件 / 样式交进来，非受管原样返回它，
// 受管版从它身上取回调（onPressed / onTap）另画一个。原型：dreamdesk/design/prototype/FileTransfer.html（样式变量 dd.css）。
import 'package:extended_text/extended_text.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/desktop/widgets/menu_button.dart';
import 'package:flutter_hbb/desktop/widgets/tabbar_widget.dart';
import 'package:flutter_hbb/models/file_model.dart';
import 'package:flutter_hbb/utils/dd_kiosk.dart';
import 'package:get/get.dart';

import 'dd_icons.dart';
import 'dd_restyle.dart';
import 'dd_theme.dart';

// 钩子的约定：名字里带「原来的那个」（original）参数、或返回可空的，函数里自己判 kDdManaged；
// ddFtSideTitle / ddFtCard / ddFtJob 是整块换掉，不判，调用方（file_manager_page.dart）要先 `if (kDdManaged)`，否则会在非受管路径画出受管外观

/// 白底 8 圆角的卡片（左右两栏、传输列表、每条传输任务）
const _kCardDecoration = BoxDecoration(color: DdColors.surface, borderRadius: BorderRadius.all(Radius.circular(DdRadius.lg)));

/// 左右两栏各自的白底卡片（原型 dd-ft__side）
BoxDecoration? get ddFtPaneDecoration => kDdManaged ? _kCardDecoration : null;

/// 路径栏底色（原型 dd-ft__path）
BoxDecoration ddFtPathDecoration(BoxDecoration original) => kDdManaged
    ? BoxDecoration(color: DdColors.pane, borderRadius: BorderRadius.circular(DdRadius.sm))
    : original;

/// 栏顶的「本地电脑 / 远程电脑」：原型只有一行粗字，不要蓝底系统图标
Widget ddFtSideTitle(String text) => SizedBox(
      height: 36,
      child: Align(
        alignment: Alignment.centerLeft,
        child: Text(text, style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600, color: DdColors.ink)),
      ),
    ).marginOnly(left: 8, bottom: 4);

/// 工具栏按钮（原型 dd-btn--small）：透明底、悬停浅灰、16 号描边图标
Widget ddFtTool(DdIcon icon, MenuButton original) =>
    kDdManaged ? _DdFtIconButton(icon: icon, tooltip: original.tooltip, onPressed: original.onPressed) : original;

/// 路径栏右边的下拉（RustDesk 用的是标签栏的 ActionIcon）
Widget ddFtActionIcon(DdIcon icon, ActionIcon original) =>
    kDdManaged ? _DdFtIconButton(icon: icon, tooltip: original.message, onPressed: original.onTap) : original;

/// 图标换成 Lucide；非受管原样返回 [original]。[padding] 只加在受管版的图标上
Widget ddFtIcon(DdIcon icon, Widget original, {double size = 16, Color color = DdColors.ink2, EdgeInsets? padding}) {
  if (!kDdManaged) return original;
  final i = ddIcon(icon, size: size, color: color);
  return padding == null ? i : Padding(padding: padding, child: i);
}

/// 列表行的图标：文件夹橙色、文件和磁盘灰色，和名字之间留 8
Widget ddFtEntryIcon(Entry entry, Widget original) {
  if (!kDdManaged) return original;
  final icon = entry.isDrive
      ? DdIcon.hardDrive
      : entry.isFile
          ? DdIcon.file
          : DdIcon.folder;
  final color = entry.isFile || entry.isDrive ? DdColors.ink2 : DdColors.folder;
  return ddIcon(icon, size: 16, color: color).marginOnly(left: 4, right: 8);
}

/// 选中行（原型 dd-ft__row--on）是浅蓝底、字照常深色，不是 RustDesk 的蓝底白字
Color ddFtRowColor(bool selected, Color original) =>
    kDdManaged ? (selected ? DdColors.accentSoft : Colors.transparent) : original;

TextStyle ddFtNameStyle(TextStyle original) =>
    kDdManaged ? const TextStyle(fontSize: 13, color: DdColors.ink) : original;

/// 修改时间、大小、表头：12 号灰字
TextStyle ddFtMetaStyle(TextStyle original) =>
    kDdManaged ? const TextStyle(fontSize: 12, color: DdColors.ink2) : original;

/// 路径栏里每一级：灰字 13 号，不要主色
ButtonStyle ddFtCrumbStyle(ButtonStyle original) => kDdManaged
    ? original.merge(const ButtonStyle(
        foregroundColor: WidgetStatePropertyAll(DdColors.ink2),
        textStyle: WidgetStatePropertyAll(TextStyle(fontSize: 13)),
        padding: WidgetStatePropertyAll(EdgeInsets.symmetric(horizontal: 6)),
      ))
    : original;

/// 「发送」主按钮、「接收」线框按钮，都是紧凑尺寸；回调就是 RustDesk 那个按钮的
Widget ddFtTransferButton(bool isLocal, ElevatedButton original) {
  if (!kDdManaged) return original;
  final enabled = original.onPressed != null;
  if (isLocal) {
    return ElevatedButton(
      onPressed: original.onPressed,
      style: ddCompactButton,
      child: Row(mainAxisSize: MainAxisSize.min, children: [
        Text(translate('Send')),
        const SizedBox(width: 6),
        ddIcon(DdIcon.arrowRight, size: 16, color: DdColors.onAccent),
      ]),
    );
  }
  return OutlinedButton(
    onPressed: original.onPressed,
    style: ddCompactButton,
    child: Row(mainAxisSize: MainAxisSize.min, children: [
      ddIcon(DdIcon.arrowLeft, size: 16, color: enabled ? DdColors.ink : DdColors.ink3),
      const SizedBox(width: 6),
      Text(translate('Receive')),
    ]),
  );
}

/// 右边传输列表的卡片：白底 8 圆角（RustDesk 是 15 圆角）
Widget ddFtCard(Widget child) => Container(decoration: _kCardDecoration, child: child);

/// 没有传输时的提示
Widget ddFtNoTransfers(Widget original) => kDdManaged
    ? Center(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          ddIcon(DdIcon.arrowLeftRight, size: 28, color: DdColors.ink3),
          const SizedBox(height: 10),
          Text(translate('No transfers in progress'),
              textAlign: TextAlign.center, style: const TextStyle(fontSize: 13, color: DdColors.ink2)),
        ]),
      )
    : original;

/// 一条传输任务（原型底部「传输」那一行的样子）：方向图标、名字、状态，进行中时一根 4 像素的进度条和百分比 / 速度；
/// 暂停时有「继续」，总有「删除」。和 RustDesk 那张卡片显示的内容、按钮一一对应
Widget ddFtJob(JobProgress item, {required VoidCallback onResume, required VoidCallback onDelete}) {
  final icon = item.type == JobType.deleteDir || item.type == JobType.deleteFile
      ? DdIcon.trash
      : item.isRemoteToLocal
          ? DdIcon.arrowLeft
          : DdIcon.arrowRight;
  final inProgress = item.type == JobType.transfer && item.state == JobState.inProgress;
  const meta = TextStyle(fontSize: 12, color: DdColors.ink2);
  return Container(
    margin: const EdgeInsets.only(bottom: 8),
    padding: const EdgeInsets.fromLTRB(12, 10, 8, 10),
    decoration: _kCardDecoration,
    child: Row(children: [
      ddIcon(icon, size: 16, color: DdColors.ink2),
      const SizedBox(width: 10),
      Expanded(
        child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
          Tooltip(
            waitDuration: const Duration(milliseconds: 500),
            message: item.jobName,
            child: ExtendedText(
              item.jobName,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(fontSize: 13, color: DdColors.ink),
              overflowWidget:
                  const TextOverflowWidget(child: Text('...'), position: TextOverflowPosition.start),
            ),
          ),
          const SizedBox(height: 2),
          Text(item.getStatus(), style: meta, maxLines: 1, overflow: TextOverflow.ellipsis),
          if (inProgress) ...[
            const SizedBox(height: 8),
            Row(children: [
              Expanded(
                child: ClipRRect(
                  borderRadius: BorderRadius.circular(2),
                  child: LinearProgressIndicator(
                      value: item.percent, minHeight: 4, backgroundColor: DdColors.selected),
                ),
              ),
              const SizedBox(width: 8),
              Text(item.recvJobRes ? '${item.percentText} · ${readableFileSize(item.speed)}/s' : item.percentText,
                  style: meta),
            ]),
          ],
        ]),
      ),
      const SizedBox(width: 4),
      if (item.state == JobState.paused)
        _DdFtIconButton(icon: DdIcon.refresh, tooltip: translate('Resume'), onPressed: onResume),
      _DdFtIconButton(icon: DdIcon.close, tooltip: translate('Delete'), onPressed: onDelete),
    ]),
  );
}

class _DdFtIconButton extends StatelessWidget {
  final DdIcon icon;
  final String? tooltip;
  final VoidCallback? onPressed;
  const _DdFtIconButton({required this.icon, this.tooltip, this.onPressed});

  @override
  Widget build(BuildContext context) {
    final on = onPressed != null;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 2),
      child: Tooltip(
        waitDuration: const Duration(milliseconds: 300),
        message: tooltip ?? '',
        child: Material(
          type: MaterialType.transparency,
          child: InkWell(
            onTap: onPressed,
            borderRadius: BorderRadius.circular(DdRadius.md),
            hoverColor: DdColors.hover,
            child: SizedBox(
              width: 28,
              height: 28,
              child: Center(child: ddIcon(icon, size: 16, color: on ? DdColors.ink : DdColors.ink3)),
            ),
          ),
        ),
      ),
    );
  }
}
