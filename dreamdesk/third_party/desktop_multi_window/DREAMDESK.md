# DreamDesk 改过的 desktop_multi_window

来源：`https://github.com/rustdesk-org/rustdesk_desktop_multi_window`，提交 `cddc2e64f8fefce4f80db87b20bcda2dd019bed6`
（就是 `flutter/pubspec.yaml` 里 `desktop_multi_window` 的 `ref`），去掉了 `example/`、`test/` 和编辑器配置。
许可是 Apache 2.0（`LICENSE` 原样保留）。`flutter/pubspec.yaml` 的 `dependency_overrides` 把它指到这里，上游那条依赖声明没动。

## 我们加的（10-27，远程窗口要最小尺寸，用户定 720 × 480）

- `setMinimumSize(Size)`：窗口能被拖到的最小尺寸（逻辑像素），只有 Windows / Linux。
  - Dart：`lib/src/window_controller.dart`、`window_controller_impl.dart`
  - Windows：`windows/base_flutter_window.h`（记最小尺寸）、`flutter_window.cc`（`WM_GETMINMAXINFO` 按窗口 DPI 换算）、
    `multi_window_manager.*`、`desktop_multi_window_plugin.cpp`（方法分发）
  - Linux：`linux/base_flutter_window.*`（`gtk_window_set_geometry_hints` 的 `GDK_HINT_MIN_SIZE`）、`multi_window_manager.*`、
    `desktop_multi_window_plugin.cc`
  - 改动处都带 `DreamDesk` 注释，搜它就能找全。

## 合并上游时

上游 RustDesk 改了 `pubspec.yaml` 里 `desktop_multi_window` 的 `ref` 时，这里要跟着换：取新版插件替换本目录（照样去掉 example / test），
再把上面这几处改动补回去，最后改这份说明里的提交号。不换的话，用的仍是这里的旧版，上游对插件的修复进不来。
