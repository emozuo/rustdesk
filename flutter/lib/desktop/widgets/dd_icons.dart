// DreamDesk 界面用的细线图标（照网易 UU 远程的观感：24 格、1.5 像素描边、圆头圆角）。Flutter 自带的 Material 图标线条
// 固定约 2 像素、改不细，所以另配一套。图形照开源图标集 Lucide（https://lucide.dev，ISC 许可，见文件末尾的许可声明）。
// 用法：ddIcon(DdIcon.settings, size: 18, color: ...)，颜色整体替换（描边是黑色占位）。
import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';

enum DdIcon {
  menu,
  user,
  info,
  arrowUpCircle,
  settings,
  devices,
  chevronUp,
  chevronDown,
  folderUp,
  terminal,
  arrowLeftRight,
  moreVertical,
  arrowRight,
  close,
  eye,
  eyeOff,
}

const _head =
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">';

const Map<DdIcon, String> _body = {
  // 横线要落在整像素上才是一条清楚的细线（落在两行像素中间会被平均成两行灰、看着又粗又虚）：按标题栏的 20 像素算，
  // 线宽 1.2 = 1 像素、y 取 (n + 0.5) / (20 / 24)
  DdIcon.menu:
      '<g stroke-width="1.2"><path d="M4 5.4h16"/><path d="M4 11.4h16"/><path d="M4 17.4h16"/></g>',
  // 照 UU 的头像：圆头 + 底边平的半身
  DdIcon.user: '<circle cx="12" cy="8" r="3.5"/><path d="M5 20.5c0-3.6 3.1-6 7-6s7 2.4 7 6z"/>',
  DdIcon.info: '<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>',
  DdIcon.arrowUpCircle: '<circle cx="12" cy="12" r="10"/><path d="m16 12-4-4-4 4"/><path d="M12 16V8"/>',
  DdIcon.settings:
      '<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>',
  DdIcon.devices:
      '<path d="M18 8V6a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h8"/><path d="M10 19v-3.96 3.15"/><path d="M7 19h5"/><rect width="6" height="10" x="16" y="12" rx="2"/>',
  DdIcon.chevronUp: '<path d="m18 15-6-6-6 6"/>',
  DdIcon.chevronDown: '<path d="m6 9 6 6 6-6"/>',
  DdIcon.folderUp:
      '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/><path d="M12 10v6"/><path d="m9 13 3-3 3 3"/>',
  DdIcon.terminal: '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="m7 11 2-2-2-2"/><path d="M11 13h4"/>',
  DdIcon.arrowLeftRight: '<path d="M8 3 4 7l4 4"/><path d="M4 7h16"/><path d="m16 21 4-4-4-4"/><path d="M20 17H4"/>',
  DdIcon.moreVertical: '<circle cx="12" cy="12" r="1"/><circle cx="12" cy="5" r="1"/><circle cx="12" cy="19" r="1"/>',
  DdIcon.arrowRight: '<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>',
  DdIcon.close: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
  DdIcon.eye: '<path d="M2.06 12.35a1 1 0 0 1 0-.7 10.75 10.75 0 0 1 19.88 0 1 1 0 0 1 0 .7 10.75 10.75 0 0 1-19.88 0"/><circle cx="12" cy="12" r="3"/>',
  DdIcon.eyeOff:
      '<path d="M10.73 5.08A10.43 10.43 0 0 1 12 5c4.4 0 8.2 2.9 9.94 6.65a1 1 0 0 1 0 .7 10.8 10.8 0 0 1-1.44 2.49"/><path d="M14.08 14.16a3 3 0 0 1-4.17-4.17"/><path d="M17.48 17.5A10.45 10.45 0 0 1 12 19c-4.4 0-8.2-2.9-9.94-6.65a1 1 0 0 1 0-.7 10.75 10.75 0 0 1 4.45-5.14"/><path d="m2 2 20 20"/>',
};

Widget ddIcon(DdIcon icon, {double size = 18, Color color = const Color(0xFF1F2329)}) => SvgPicture.string(
      '$_head${_body[icon]}</svg>',
      width: size,
      height: size,
      colorFilter: ColorFilter.mode(color, BlendMode.srcIn),
    );

// Lucide 许可声明（ISC License）：
// Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT). All other copyright (c) for
// Lucide are held by Lucide Contributors 2022.
// Permission to use, copy, modify, and/or distribute this software for any purpose with or without fee is hereby granted,
// provided that the above copyright notice and this permission notice appear in all copies.
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED
// WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR
// CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
// NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
