// 受管版远程窗口里几处弹窗和提示的样子（用户 10-27 定方案 A：只换外观，不动行为）。
// RustDesk 原处只留 `if (kDdManaged) ...` 薄钩子，取值、存值、按钮回调照用它的，这里只管画。
// 原型：dreamdesk/design/prototype/RemoteDialogs.html、Dialogs.html（样式变量 dd.css）。
import 'package:debounce_throttle/debounce_throttle.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:get/get.dart';

import 'dd_restyle.dart';
import 'dd_theme.dart';

/// showToast 的受管版（原型 dd-toast）：深色半透明 6 圆角条、13 号白字。显示多久、摆在哪照调用方的
void ddShowToast(String text, {required Duration timeout, required Alignment alignment}) {
  final overlayState = globalKey.currentState?.overlay;
  if (overlayState == null) return;
  final entry = OverlayEntry(
    builder: (context) => IgnorePointer(
      child: Align(
        alignment: alignment,
        child: Container(
          constraints: const BoxConstraints(minHeight: 38),
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
          decoration: BoxDecoration(color: DdColors.ink.withAlpha(0xE6), borderRadius: BorderRadius.circular(DdRadius.md)),
          child: Text(text,
              textAlign: TextAlign.center,
              style: const TextStyle(
                  decoration: TextDecoration.none, fontSize: 13, fontWeight: FontWeight.normal, color: DdColors.onAccent)),
        ),
      ),
    ),
  );
  overlayState.insert(entry);
  Future.delayed(timeout, () => entry.remove());
}

/// 截图完成的弹窗正文（原型「工具 › 截图」）：标题前一个绿色对勾——就是 ddMsgboxContent 的「成功」那一款。
/// RustDesk 原来用的弹窗类型 custom-nook-nocancel-hasclose 不带图标，所以这里单独给它指定类型
Widget ddScreenshotContent(String title, String text) => ddMsgboxContent('success', title, translate(text));

/// customImageQualityWidget 的受管版（原型「画质 › 自定义」）：每项一行「名称 …… 当前值」，下面一根细滑块；
/// 「更多」（放大码率上限）是滑块下面一个复选框。取值范围、去抖、回调和 RustDesk 那份一样
Widget ddCustomImageQualityWidget(
    {required double initQuality,
    required double initFps,
    required Function(double)? setQuality,
    required Function(double)? setFps,
    required bool showFps,
    required bool showMoreQuality}) {
  if (initQuality < kMinQuality || initQuality > (showMoreQuality ? kMaxMoreQuality : kMaxQuality)) {
    initQuality = kDefaultQuality;
  }
  if (initFps < kMinFps || initFps > kMaxFps) {
    initFps = kDefaultFps;
  }
  final qualityValue = initQuality.obs;
  final fpsValue = initFps.obs;
  final moreQualityChecked = RxBool(qualityValue.value > kMaxQuality);
  final debouncerQuality =
      Debouncer<double>(const Duration(milliseconds: 1000), onChanged: setQuality, initialValue: qualityValue.value);
  final debouncerFps =
      Debouncer<double>(const Duration(milliseconds: 1000), onChanged: setFps, initialValue: fpsValue.value);

  onMoreChanged(bool? value) {
    if (value == null) return;
    moreQualityChecked.value = value;
    if (!value && qualityValue.value > 100) {
      qualityValue.value = 100;
    }
    debouncerQuality.value = qualityValue.value;
  }

  return SizedBox(
    width: 360,
    child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
      Obx(() => _slider(
            label: translate('Bitrate'),
            value: '${qualityValue.value.round()}%',
            slider: Slider(
              value: qualityValue.value,
              min: kMinQuality,
              max: moreQualityChecked.value ? kMaxMoreQuality : kMaxQuality,
              divisions: moreQualityChecked.value
                  ? ((kMaxMoreQuality - kMinQuality) / 10).round()
                  : ((kMaxQuality - kMinQuality) / 5).round(),
              onChanged: setQuality == null
                  ? null
                  : (double value) {
                      qualityValue.value = value;
                      debouncerQuality.value = value;
                    },
            ),
          )),
      if (showFps)
        Obx(() => _slider(
              label: translate('FPS'),
              value: '${fpsValue.value.round()} fps',
              slider: Slider(
                value: fpsValue.value,
                min: kMinFps,
                max: kMaxFps,
                divisions: ((kMaxFps - kMinFps) / 5).round(),
                onChanged: setFps == null
                    ? null
                    : (double value) {
                        fpsValue.value = value;
                        debouncerFps.value = value;
                      },
              ),
            )),
      if (showMoreQuality)
        Obx(() => InkWell(
              onTap: () => onMoreChanged(!moreQualityChecked.value),
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                SizedBox(
                  width: 18,
                  height: 18,
                  child: Checkbox(
                      value: moreQualityChecked.value,
                      onChanged: onMoreChanged,
                      materialTapTargetSize: MaterialTapTargetSize.shrinkWrap),
                ),
                const SizedBox(width: 8),
                Text(translate('More'), style: const TextStyle(fontSize: 13, color: DdColors.ink)),
              ]),
            )),
    ]),
  );
}

Widget _slider({required String label, required String value, required Widget slider}) => Padding(
      padding: const EdgeInsets.only(bottom: 14),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisSize: MainAxisSize.min, children: [
        Row(children: [
          Expanded(child: Text(label, style: const TextStyle(fontSize: 13, color: DdColors.ink))),
          Text(value, style: const TextStyle(fontSize: 13, color: DdColors.ink2)),
        ]),
        const SizedBox(height: 6),
        // 原型是一根 4 像素的进度条；滑块要能拖，留一个小圆钮，刻度点不画
        SliderTheme(
          data: SliderThemeData(
            trackHeight: 4,
            activeTrackColor: DdColors.accent,
            inactiveTrackColor: DdColors.selected,
            disabledActiveTrackColor: DdColors.ink3,
            disabledInactiveTrackColor: DdColors.selected,
            thumbColor: DdColors.accent,
            disabledThumbColor: DdColors.ink3,
            overlayColor: DdColors.accentSoft,
            thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 7),
            overlayShape: const RoundSliderOverlayShape(overlayRadius: 14),
            tickMarkShape: SliderTickMarkShape.noTickMark,
            trackShape: const RoundedRectSliderTrackShape(),
            showValueIndicator: ShowValueIndicator.never,
          ),
          child: SizedBox(height: 24, child: slider),
        ),
      ]),
    );
