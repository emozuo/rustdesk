// 受管版远程画面的滚动条（滚动方式选「滚动条」或「边缘滚动」、画面比窗口大时；原型 dreamdesk/design/prototype/Remote.html 最下面一节、
// dd.css 的 .dd-rsb-*，照 UU 截图量的）：右 / 下各一条 10 宽的浅灰轨道，轨道正中一道 6 宽的更浅的槽（圆头），滑块 6 宽圆头正好填满这道槽；
// 两条在右下角让开 10；一直显示。RustDesk 原来是 12 宽方头灰条、滚动时才出现（remote_page.dart 的 _buildCrossScrollbarFromLayout）。
// 拖动、点轨道翻页、和画面滚动的联动都还是 RawScrollbar 的，这里只给它参数，轨道和槽画在它下面
import 'package:flutter/material.dart';

import 'dd_theme.dart';

const _kTrack = 10.0;
const _kInset = 2.0;

/// [child] 是 RustDesk 搭好的画面（横向 / 纵向的 SingleChildScrollView 套在一起）；[horizontal] / [vertical] 为空表示那个方向不溢出、没有滚动条。
/// 两个方向都溢出时画面右 / 下各让出 10 给轨道（画面不被挡；滚动范围跟着多出 10，鼠标坐标按滚动像素算、不受影响）。
/// 只有一个方向溢出时不让：另一个方向的画面是在整宽 / 整高里居中的（RustDesk 换算鼠标坐标时也按整宽 / 整高算），让了会差 5 像素；
/// 这时轨道压在画面居中留出的空白上，画面几乎和窗口一样宽 / 高时会压住边上不到 10 像素。
/// 「滚动条」「边缘滚动」两种方式都用这一套（RustDesk 也是同一套滚动条，边缘滚动只多了碰边自动滚；它的触发区默认 100 宽，轨道压住的 10 不碍事）
class DdRemoteScrollbars extends StatefulWidget {
  final Widget child;
  final ScrollController? horizontal;
  final ScrollController? vertical;
  const DdRemoteScrollbars({super.key, required this.child, this.horizontal, this.vertical});

  @override
  State<DdRemoteScrollbars> createState() => _DdRemoteScrollbarsState();
}

class _DdRemoteScrollbarsState extends State<DdRemoteScrollbars> {
  bool _hotH = false;
  bool _hotV = false;

  Widget _track() => Container(
        color: DdColors.remoteScrollTrack,
        padding: const EdgeInsets.all(_kInset),
        child: const DecoratedBox(
            decoration: BoxDecoration(
                color: DdColors.remoteScrollGroove, borderRadius: BorderRadius.all(Radius.circular(_kTrack / 2 - _kInset)))),
      );

  /// 鼠标在哪条轨道上。只能在最外面按位置算：轨道可见时 RawScrollbar 的前景画笔把轨道上的命中整个截走，
  /// 轨道上挂的 MouseRegion 收不到进出
  void _hover(Offset? pos, {required bool h, required bool v}) {
    final size = context.size;
    if (size == null) return;
    final hotV = pos != null && v && pos.dx >= size.width - _kTrack && (!h || pos.dy < size.height - _kTrack);
    final hotH = pos != null && h && pos.dy >= size.height - _kTrack && (!v || pos.dx < size.width - _kTrack);
    if (hotV != _hotV || hotH != _hotH) {
      setState(() {
        _hotV = hotV;
        _hotH = hotH;
      });
    }
  }

  RawScrollbar _bar(Widget child, ScrollController controller,
          {required bool hot, required EdgeInsets padding, ScrollNotificationPredicate? predicate}) =>
      RawScrollbar(
        controller: controller,
        thumbVisibility: true,
        // 轨道要「可见」，点轨道翻页才生效；颜色透明，看到的是下面自己画的轨道和槽
        trackVisibility: true,
        trackColor: Colors.transparent,
        trackBorderColor: Colors.transparent,
        thickness: _kTrack - 2 * _kInset,
        radius: const Radius.circular(_kTrack / 2 - _kInset),
        thumbColor: hot ? DdColors.remoteScrollThumbHot : DdColors.remoteScrollThumb,
        crossAxisMargin: _kInset,
        mainAxisMargin: _kInset,
        padding: padding,
        notificationPredicate: predicate ?? defaultScrollNotificationPredicate,
        child: child,
      );

  @override
  Widget build(BuildContext context) {
    final h = widget.horizontal;
    final v = widget.vertical;
    final both = h != null && v != null;
    Widget w = Stack(fit: StackFit.expand, children: [
      Padding(padding: EdgeInsets.only(right: both ? _kTrack : 0, bottom: both ? _kTrack : 0), child: widget.child),
      if (v != null) Positioned(right: 0, top: 0, bottom: h != null ? _kTrack : 0, width: _kTrack, child: _track()),
      if (h != null) Positioned(left: 0, bottom: 0, right: v != null ? _kTrack : 0, height: _kTrack, child: _track()),
      if (both)
        const Positioned(
            right: 0, bottom: 0, width: _kTrack, height: _kTrack, child: ColoredBox(color: DdColors.remoteScrollTrack)),
    ]);
    if (h != null) {
      // 两个方向都滚时横向的那个 SingleChildScrollView 套在纵向的里面，深一层（照 RustDesk 原来的判断）
      w = _bar(w, h,
          hot: _hotH,
          padding: EdgeInsets.only(right: v != null ? _kTrack : 0),
          predicate: v != null ? (n) => n.depth == 1 : null);
    }
    if (v != null) {
      w = _bar(w, v, hot: _hotV, padding: EdgeInsets.only(bottom: h != null ? _kTrack : 0));
    }
    return MouseRegion(
      opaque: false,
      onHover: (e) => _hover(e.localPosition, h: h != null, v: v != null),
      onExit: (_) => _hover(null, h: false, v: false),
      child: w,
    );
  }
}
