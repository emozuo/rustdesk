// DreamDesk 桌面端界面规范落到代码（照网易 UU 远程；规范以本地原型为准：dreamdesk/design/prototype，样式变量在 dd.css）。
// 颜色、尺寸只在这里定义一次，名字和规范里的 token 一一对应；我们自己的界面（首页、远程窗口标题栏、控制中心、悬浮球）从这里取。
// [ddLightTheme] 是产品版的整体主题：在 RustDesk 的浅色主题上换掉规范管到的部分（配色、按钮、输入框、菜单、对话框、复选 / 单选 / 开关、
// 分隔线、提示框），RustDesk 原有的设置页、对话框、文件传输等一次换掉，不逐页改它们。产品版先固定浅色。
import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:xterm/xterm.dart';

class DdColors {
  DdColors._();
  // 10-27 照 UU 截图取色（原型 dreamdesk/design/prototype/dd.css）：不是纯灰，带一点蓝
  static const chrome = Color(0xFFF0F5F8);
  static const pane = Color(0xFFF7FBFE);
  static const surface = Color(0xFFFFFFFF);
  static const selected = Color(0xFFE4E9EC);
  /// 内容区上边、左边和左栏之间的细线
  static const paneLine = Color(0xFFEAEEF1);
  /// 左栏底部「设置」上方的分隔线
  static const sideDivider = Color(0xFFE4E9EC);
  /// 远程窗口的标题栏（UU 的远程窗口比主窗口深一档）
  static const chromeRemote = Color(0xFFCDD3D8);
  // 远程窗口和子窗口标题栏（10-27 用户对照 UU 截图，原型 Remote.html）：字小一号更深；没选中的标签、麦克风、控制中心
  // 悬停是一块圆角深色底；窗口按钮悬停浅一档、占满高；分隔竖线在深一档的底上要看得见；关闭按钮悬停红底
  static const titleText = Color(0xFF111418);
  static const tabHover = Color(0x26000000);
  static const winHover = Color(0x12000000);
  static const titleSep = Color(0xFFB4BAC0);
  static const closeHover = Color(0xFFE81123);
  static const hover = Color(0x0F000000);
  static const line = Color(0xFFDCDFE4);
  static const lineStrong = Color(0xFF8A9099);
  static const ink = Color(0xFF1F2329);
  static const ink2 = Color(0xFF4E5560);
  static const ink3 = Color(0xFF9AA0A8);
  // 10-28 取自 UU 截图的按钮蓝；悬停 / 按下照原来的比例压暗
  static const accent = Color(0xFF3A7BFB);
  static const accentHover = Color(0xFF346FE2);
  static const accentPress = Color(0xFF2E62C9);
  static const accentSoft = Color(0xFFE8F0FE);
  /// 浅蓝底（accentSoft）上的细边（控制中心「允许对方」里开着的那几个；原型 dd-cm--on）
  static const accentBorder = Color(0xFFB9CDF7);
  static const onAccent = Color(0xFFFFFFFF);
  /// 主按钮禁用时的底
  static const disabledFill = Color(0xFFC9CDD4);
  static const online = Color(0xFF22C55E);
  static const mine = Color(0xFF14B8A6);
  static const busy = Color(0xFFF97316);
  static const offline = Color(0xFF9AA0A8);
  /// 用不了的操作（本机详情里的文件传输 / 终端 / 端口映射，UU 的灰；原型 HostCollapsed）
  static const unavailable = Color(0xFF9B9DA1);
  /// 首页「被控中」（本机正连着这台，UU 的蓝；原型 HomeControlling）：横条底、边、主字、副字和中间竖线，左栏会话数角标的框
  static const controlBg = Color(0xFFDEEFFF);
  static const controlBorder = Color(0xFFC4DEFD);
  static const controlText = Color(0xFF2B7AF0);
  static const controlSub = Color(0xFF4A8EF0);
  static const controlDivider = Color(0xFFB3D2FB);
  static const controlBadge = Color(0xFF8FB6F2);
  /// 会话类型小标签的灰框（原型 HomeControllingOpen）
  static const tagBorder = Color(0xFFC9CDD4);
  /// 「本机被控中」面板（被控端，原型 HostPanel / HostCollapsed）：展开的大面板底、收起的左栏卡片横向渐变两端、连接行的细边
  static const hostBlue = Color(0xFF3183FF);
  static const hostCardFrom = Color(0xFF338BFF);
  static const hostCardTo = Color(0xFF4B99FF);
  static const hostRowBorder = Color(0xFFEBEEF3);
  /// 远程画面左上角模式提示条的灰底（通用模式，原型 RemoteDialogs）
  static const modeBadge = Color(0xFF57575E);
  /// 文件传输列表里的文件夹图标（原型 FileTransfer）
  static const folder = Color(0xFFF0A020);
  /// 弹窗里「成功」的图标色（原型 Dialogs）
  static const success = Color(0xFF22A35A);
  static const danger = Color(0xFFE5484D);
  static const dangerText = Color(0xFFD13438);
  static const pill = Color(0xFF1F2329);
  static const osWindows = Color(0xFF3B7BF6);
  static const osLinux = Color(0xFFF08A24);
  static const osMac = Color(0xFF1F2329);
  static const ball = Color(0xF7F1F1F2);
  static const ballBand = Color(0xFFE3E3E6);
  static const ballBandHot = Color(0xFFB1CBFF);
  static const ballInk = Color(0xFF474D57);
  static const scrim = Color(0x66000000);
  /// 提示条的暖色底（端口映射的绿条、主窗口「连不上服务器」条；原型 dd-tip--warn / dd-netwarn）
  static const warnBg = Color(0xFFFDF0E6);
  /// 滚动条滑块（照 UU / Win11 的细滚动条，原型 dd.css）：平时半透明灰，悬停 / 拖动加深
  static const scrollThumb = Color(0x24000000);
  static const scrollThumbHot = Color(0x73000000);
  /// 远程画面的滚动条（照 UU 截图量的，原型 Remote.html 最下面一节）：轨道、轨道正中的浅槽、滑块、滑块悬停 / 拖动
  static const remoteScrollTrack = Color(0xFFF0F0F0);
  static const remoteScrollGroove = Color(0xFFFAFAFA);
  static const remoteScrollThumb = Color(0xFFB4B4B4);
  static const remoteScrollThumbHot = Color(0xFF5C5C5C);
}

class DdRadius {
  DdRadius._();
  static const sm = 4.0;
  static const md = 6.0;
  static const lg = 8.0;
  static const xl = 10.0;
  static const ball = 13.0;
  static const pill = 12.0;
}

class DdSize {
  DdSize._();
  static const titlebarMain = 48.0;
  static const titlebarRemote = 40.0;
  static const sidebarWidth = 236.0;
  static const rowHeight = 40.0;
  static const buttonHeight = 36.0;
  static const ballSize = 42.0;
  /// 远程窗口最小尺寸（10-27 用户；原型 Remote.html：720 宽时三个只剩图标的标签和标题栏其余部分正好放下）
  static const remoteWindowMin = Size(720, 480);
}

class DdShadow {
  DdShadow._();
  static const card = [BoxShadow(color: Color(0x14000000), blurRadius: 10, offset: Offset(0, 2))];
  static const dialog = [BoxShadow(color: Color(0x33000000), blurRadius: 30, offset: Offset(0, 10))];
  static const ball = [BoxShadow(color: Color(0x33000000), blurRadius: 10, offset: Offset(0, 2))];
}

/// 终端配色（原型 Terminal.html）：底 #1e2127、字 #d7dae0、光标是同色的块；16 色照 xterm 的默认（VS Code 那套）
const ddTerminalTheme = TerminalTheme(
  cursor: Color(0xFFD7DAE0),
  selection: Color(0x663B8EEA),
  foreground: Color(0xFFD7DAE0),
  background: Color(0xFF1E2127),
  black: Color(0xFF000000),
  red: Color(0xFFCD3131),
  green: Color(0xFF0DBC79),
  yellow: Color(0xFFE5E510),
  blue: Color(0xFF2472C8),
  magenta: Color(0xFFBC3FBC),
  cyan: Color(0xFF11A8CD),
  white: Color(0xFFE5E5E5),
  brightBlack: Color(0xFF666666),
  brightRed: Color(0xFFF14C4C),
  brightGreen: Color(0xFF23D18B),
  brightYellow: Color(0xFFF5F543),
  brightBlue: Color(0xFF3B8EEA),
  brightMagenta: Color(0xFFD670D6),
  brightCyan: Color(0xFF29B8DB),
  brightWhite: Color(0xFFFFFFFF),
  searchHitBackground: Color(0xFFFFFF2B),
  searchHitBackgroundCurrent: Color(0xFF31FF26),
  searchHitForeground: Color(0xFF000000),
);

RoundedRectangleBorder _round(double r) => RoundedRectangleBorder(borderRadius: BorderRadius.circular(r));

ThemeData? _ddLightTheme;

// 10-27 在本机和 UU 对比试过思源黑体、黑体、宋体、等线、更纱黑体 UI、HarmonyOS Sans，都不比系统默认的微软雅黑 UI 更清楚
// （发虚是 Flutter 不用 ClearType 的灰度抗锯齿，换字体解决不了），定为雅黑 UI
final _cjkFallback = isWindows ? const ['Microsoft YaHei UI', 'Microsoft YaHei'] : const ['Noto Sans CJK SC'];

/// 产品版的整体主题（浅色）：在 RustDesk 浅色主题上换掉规范管到的部分，它自己的主题扩展（ColorThemeExtension 等）照留
ThemeData ddLightTheme() => _ddLightTheme ??= _build();

ThemeData _build() {
  final base = MyTheme.lightTheme;
  const body = TextStyle(fontSize: 14, color: DdColors.ink);
  final inputBorder = OutlineInputBorder(
      borderRadius: BorderRadius.circular(DdRadius.sm), borderSide: const BorderSide(color: DdColors.line));
  WidgetStateProperty<Color?> selectedOr(Color on, Color? off) =>
      WidgetStateProperty.resolveWith((s) => s.contains(WidgetState.selected) ? on : off);
  return base.copyWith(
    // 点击不要水波纹，只留悬停底色
    splashFactory: NoSplash.splashFactory,
    splashColor: Colors.transparent,
    highlightColor: Colors.transparent,
    hoverColor: DdColors.hover,
    scaffoldBackgroundColor: DdColors.pane,
    dialogBackgroundColor: DdColors.surface,
    cardColor: DdColors.surface,
    dividerColor: DdColors.line,
    hintColor: DdColors.ink3,
    colorScheme: base.colorScheme.copyWith(
      primary: DdColors.accent,
      onPrimary: DdColors.onAccent,
      secondary: DdColors.accent,
      onSecondary: DdColors.onAccent,
      // RustDesk 的页面还用 colorScheme.background 当底色（远程窗口、设置页），要一起换
      // ignore: deprecated_member_use
      background: DdColors.pane,
      surface: DdColors.surface,
      error: DdColors.dangerText,
    ),
    // 中文写明用哪个字体：不写时由 Flutter 按系统挑，可能挑到笔画细的（等线），在它的灰度抗锯齿下更发虚；
    // Windows 用为界面小字号设计的微软雅黑 UI，Linux 照 kiosk 用 Noto Sans CJK。西文仍是系统默认（Segoe UI 等）
    textTheme: base.textTheme.apply(
        bodyColor: DdColors.ink, displayColor: DdColors.ink, fontFamilyFallback: _cjkFallback),
    textSelectionTheme: const TextSelectionThemeData(
        cursorColor: DdColors.accent, selectionColor: DdColors.accentSoft, selectionHandleColor: DdColors.accent),
    dividerTheme: const DividerThemeData(color: DdColors.line),
    dialogTheme: DialogTheme(
      backgroundColor: DdColors.surface,
      surfaceTintColor: DdColors.surface,
      shape: _round(DdRadius.xl),
      titleTextStyle: const TextStyle(fontSize: 18, fontWeight: FontWeight.w600, color: DdColors.ink),
      contentTextStyle: body,
    ),
    inputDecorationTheme: base.inputDecorationTheme.copyWith(
      filled: true,
      fillColor: DdColors.surface,
      hintStyle: const TextStyle(color: DdColors.ink3),
      border: inputBorder,
      enabledBorder: inputBorder,
      focusedBorder: inputBorder.copyWith(borderSide: const BorderSide(color: DdColors.accent)),
      errorBorder: inputBorder.copyWith(borderSide: const BorderSide(color: DdColors.danger)),
    ),
    elevatedButtonTheme: ElevatedButtonThemeData(
      style: ElevatedButton.styleFrom(
        foregroundColor: DdColors.onAccent,
        disabledForegroundColor: DdColors.onAccent,
        elevation: 0,
        minimumSize: const Size(0, DdSize.buttonHeight),
        padding: const EdgeInsets.symmetric(horizontal: 24),
        shape: _round(DdRadius.sm),
      ).copyWith(
        backgroundColor: WidgetStateProperty.resolveWith((s) => s.contains(WidgetState.disabled)
            ? DdColors.disabledFill
            : s.contains(WidgetState.pressed)
                ? DdColors.accentPress
                : s.contains(WidgetState.hovered)
                    ? DdColors.accentHover
                    : DdColors.accent),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        backgroundColor: DdColors.surface,
        foregroundColor: DdColors.ink,
        side: const BorderSide(color: DdColors.line),
        minimumSize: const Size(0, DdSize.buttonHeight),
        padding: const EdgeInsets.symmetric(horizontal: 24),
        shape: _round(DdRadius.sm),
      ),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(foregroundColor: DdColors.accent, shape: _round(DdRadius.sm)),
    ),
    checkboxTheme: base.checkboxTheme.copyWith(
      fillColor: selectedOr(DdColors.accent, Colors.transparent),
      checkColor: const WidgetStatePropertyAll(DdColors.onAccent),
      side: const BorderSide(color: DdColors.lineStrong, width: 1.5),
      shape: _round(3),
    ),
    radioTheme: base.radioTheme.copyWith(fillColor: selectedOr(DdColors.accent, DdColors.lineStrong)),
    switchTheme: base.switchTheme.copyWith(
      thumbColor: const WidgetStatePropertyAll(DdColors.surface),
      trackColor: selectedOr(DdColors.accent, DdColors.disabledFill),
    ),
    progressIndicatorTheme: const ProgressIndicatorThemeData(color: DdColors.accent),
    // 细滚动条：6 宽圆头、没有轨道、离边 2。悬停时不加粗（Material 默认悬停变 12 宽）。远程画面的滚动条自己写了一套（RawScrollbar），不受这里管
    scrollbarTheme: ScrollbarThemeData(
      thickness: const WidgetStatePropertyAll(6),
      radius: const Radius.circular(3),
      thumbColor: WidgetStateProperty.resolveWith((s) =>
          s.contains(WidgetState.dragged) || s.contains(WidgetState.hovered) ? DdColors.scrollThumbHot : DdColors.scrollThumb),
      crossAxisMargin: 2,
      mainAxisMargin: 2,
    ),
    popupMenuTheme: base.popupMenuTheme.copyWith(color: DdColors.surface, elevation: 8, shape: _round(DdRadius.lg)),
    menuTheme: const MenuThemeData(
      style: MenuStyle(
        backgroundColor: WidgetStatePropertyAll(DdColors.surface),
        surfaceTintColor: WidgetStatePropertyAll(DdColors.surface),
        elevation: WidgetStatePropertyAll(8),
        shape: WidgetStatePropertyAll(RoundedRectangleBorder(borderRadius: BorderRadius.all(Radius.circular(DdRadius.lg)))),
        padding: WidgetStatePropertyAll(EdgeInsets.symmetric(vertical: 6)),
      ),
    ),
    tooltipTheme: const TooltipThemeData(
      decoration: BoxDecoration(color: DdColors.ink, borderRadius: BorderRadius.all(Radius.circular(DdRadius.sm))),
      textStyle: TextStyle(fontSize: 12, color: DdColors.onAccent),
    ),
  );
}
