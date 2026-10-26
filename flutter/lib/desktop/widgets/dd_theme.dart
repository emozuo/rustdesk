// DreamDesk 桌面端界面规范落到代码（规范：https://claude.ai/artifact/F1ALcvqqEKjcnmkLoyLSNK，用户 10-26 确认，照网易 UU 远程）。
// 颜色、尺寸只在这里定义一次，名字和规范里的 token 一一对应；我们自己的界面（首页、远程窗口标题栏、控制中心、悬浮球）从这里取。
// [ddLightTheme] 是产品版的整体主题：在 RustDesk 的浅色主题上换掉规范管到的部分（配色、按钮、输入框、菜单、对话框、复选 / 单选 / 开关、
// 分隔线、提示框），RustDesk 原有的设置页、对话框、文件传输等一次换掉，不逐页改它们。产品版先固定浅色。
import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';

class DdColors {
  DdColors._();
  static const chrome = Color(0xFFEDEFF3);
  static const pane = Color(0xFFF7F8FA);
  static const surface = Color(0xFFFFFFFF);
  static const selected = Color(0xFFE1E4E9);
  static const hover = Color(0x0F000000);
  static const line = Color(0xFFDCDFE4);
  static const lineStrong = Color(0xFF8A9099);
  static const ink = Color(0xFF1F2329);
  static const ink2 = Color(0xFF646A73);
  static const ink3 = Color(0xFF9AA0A8);
  static const accent = Color(0xFF2F6FEA);
  static const accentHover = Color(0xFF2A63D2);
  static const accentPress = Color(0xFF2457BD);
  static const accentSoft = Color(0xFFE8F0FE);
  static const onAccent = Color(0xFFFFFFFF);
  /// 主按钮禁用时的底
  static const disabledFill = Color(0xFFC9CDD4);
  static const online = Color(0xFF22C55E);
  static const mine = Color(0xFF14B8A6);
  static const busy = Color(0xFFF97316);
  static const offline = Color(0xFF9AA0A8);
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
}

class DdShadow {
  DdShadow._();
  static const card = [BoxShadow(color: Color(0x14000000), blurRadius: 10, offset: Offset(0, 2))];
  static const menu = [BoxShadow(color: Color(0x1F000000), blurRadius: 16, offset: Offset(0, 4))];
  static const dialog = [BoxShadow(color: Color(0x33000000), blurRadius: 30, offset: Offset(0, 10))];
  static const ball = [BoxShadow(color: Color(0x33000000), blurRadius: 10, offset: Offset(0, 2))];
}

RoundedRectangleBorder _round(double r) => RoundedRectangleBorder(borderRadius: BorderRadius.circular(r));

ThemeData? _ddLightTheme;

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
    textTheme: base.textTheme.apply(bodyColor: DdColors.ink, displayColor: DdColors.ink),
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
        backgroundColor: DdColors.accent,
        foregroundColor: DdColors.onAccent,
        disabledBackgroundColor: DdColors.disabledFill,
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
