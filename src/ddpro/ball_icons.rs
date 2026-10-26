//! 专业窗口悬浮球的图标：全部是开源图标库的原图（清单见 dreamdesk/design/图标清单.md），本文件由 dreamdesk/scripts/gen-ball-icons.py 从原文件生成，不手改。
//! SVG 的路径、圆、矩形、直线、折线换算成绝对坐标的 移到 / 直线 / 三次曲线 / 闭合（圆弧按 SVG 规范换成三次曲线逼近，
//! 形状不改），由 `ball.rs` 用 tiny-skia 画。
//! - Lucide（lucide-static 0.460.0，ISC 许可）：描边图标，24 格，线宽按界面规范 1.5
//! - Fluent UI System Icons（@fluentui/svg-icons 1.1.292，MIT 许可）：填色图标，20 号原图
//! 两个库的许可声明原文在文件末尾。

/// 一段路径指令（坐标在图标自己的格子里）
pub enum Seg {
    M(f32, f32),
    L(f32, f32),
    C(f32, f32, f32, f32, f32, f32),
    Z,
}

/// 图标：格子边长（Lucide 24、Fluent 20 号是 20）、填色还是描边、路径
pub struct Icon {
    pub grid: f32,
    pub fill: bool,
    pub segs: &'static [Seg],
}

use Seg::{C, L, M, Z};

/// fluent toggle_multiple_20_regular
pub const BALL: Icon = Icon {
    grid: 20.0,
    fill: true,
    segs: &[
        M(8.0, 5.5),
        C(8.0, 6.3284, 7.3284, 7.0, 6.5, 7.0),
        C(5.6716, 7.0, 5.0, 6.3284, 5.0, 5.5),
        C(5.0, 4.6716, 5.6716, 4.0, 6.5, 4.0),
        C(7.3284, 4.0, 8.0, 4.6716, 8.0, 5.5),
        Z,
        M(3.0, 5.5),
        C(3.0, 7.433, 4.567, 9.0, 6.5, 9.0),
        L(13.5, 9.0),
        C(15.433, 9.0, 17.0, 7.433, 17.0, 5.5),
        C(17.0, 3.567, 15.433, 2.0, 13.5, 2.0),
        L(6.5, 2.0),
        C(4.567, 2.0, 3.0, 3.567, 3.0, 5.5),
        Z,
        M(6.5, 3.0),
        L(13.5, 3.0),
        C(14.8807, 3.0, 16.0, 4.1193, 16.0, 5.5),
        C(16.0, 6.8807, 14.8807, 8.0, 13.5, 8.0),
        L(6.5, 8.0),
        C(5.1193, 8.0, 4.0, 6.8807, 4.0, 5.5),
        C(4.0, 4.1193, 5.1193, 3.0, 6.5, 3.0),
        Z,
        M(13.5, 16.0),
        C(14.3284, 16.0, 15.0, 15.3284, 15.0, 14.5),
        C(15.0, 13.6716, 14.3284, 13.0, 13.5, 13.0),
        C(12.6716, 13.0, 12.0, 13.6716, 12.0, 14.5),
        C(12.0, 15.3284, 12.6716, 16.0, 13.5, 16.0),
        Z,
        M(3.0, 14.5),
        C(3.0, 16.433, 4.567, 18.0, 6.5, 18.0),
        L(13.5, 18.0),
        C(15.433, 18.0, 17.0, 16.433, 17.0, 14.5),
        C(17.0, 12.567, 15.433, 11.0, 13.5, 11.0),
        L(6.5, 11.0),
        C(4.567, 11.0, 3.0, 12.567, 3.0, 14.5),
        Z,
        M(6.5, 12.0),
        L(13.5, 12.0),
        C(14.8807, 12.0, 16.0, 13.1193, 16.0, 14.5),
        C(16.0, 15.8807, 14.8807, 17.0, 13.5, 17.0),
        L(6.5, 17.0),
        C(5.1193, 17.0, 4.0, 15.8807, 4.0, 14.5),
        C(4.0, 13.1193, 5.1193, 12.0, 6.5, 12.0),
        Z,
    ],
};

/// lucide monitor
pub const MONITOR: Icon = Icon {
    grid: 24.0,
    fill: false,
    segs: &[
        M(4.0, 3.0),
        L(20.0, 3.0),
        C(21.1046, 3.0, 22.0, 3.8954, 22.0, 5.0),
        L(22.0, 15.0),
        C(22.0, 16.1046, 21.1046, 17.0, 20.0, 17.0),
        L(4.0, 17.0),
        C(2.8954, 17.0, 2.0, 16.1046, 2.0, 15.0),
        L(2.0, 5.0),
        C(2.0, 3.8954, 2.8954, 3.0, 4.0, 3.0),
        Z,
        M(8.0, 21.0),
        L(16.0, 21.0),
        M(12.0, 17.0),
        L(12.0, 21.0),
    ],
};

/// lucide maximize-2
pub const RESOLUTION: Icon = Icon {
    grid: 24.0,
    fill: false,
    segs: &[
        M(15.0, 3.0),
        L(21.0, 3.0),
        L(21.0, 9.0),
        M(9.0, 21.0),
        L(3.0, 21.0),
        L(3.0, 15.0),
        M(21.0, 3.0),
        L(14.0, 10.0),
        M(3.0, 21.0),
        L(10.0, 14.0),
    ],
};

/// lucide signal
pub const SIGNAL: Icon = Icon {
    grid: 24.0,
    fill: false,
    segs: &[
        M(2.0, 20.0),
        L(2.01, 20.0),
        M(7.0, 20.0),
        L(7.0, 16.0),
        M(12.0, 20.0),
        L(12.0, 12.0),
        M(17.0, 20.0),
        L(17.0, 8.0),
        M(22.0, 4.0),
        L(22.0, 20.0),
    ],
};

/// lucide mic
pub const MIC: Icon = Icon {
    grid: 24.0,
    fill: false,
    segs: &[
        M(12.0, 2.0),
        C(10.3431, 2.0, 9.0, 3.3431, 9.0, 5.0),
        L(9.0, 12.0),
        C(9.0, 13.6569, 10.3431, 15.0, 12.0, 15.0),
        C(13.6569, 15.0, 15.0, 13.6569, 15.0, 12.0),
        L(15.0, 5.0),
        C(15.0, 3.3431, 13.6569, 2.0, 12.0, 2.0),
        Z,
        M(19.0, 10.0),
        L(19.0, 12.0),
        C(19.0, 15.866, 15.866, 19.0, 12.0, 19.0),
        C(8.134, 19.0, 5.0, 15.866, 5.0, 12.0),
        L(5.0, 10.0),
        M(12.0, 19.0),
        L(12.0, 22.0),
    ],
};

/// lucide sun
pub const HDR: Icon = Icon {
    grid: 24.0,
    fill: false,
    segs: &[
        M(16.0, 12.0),
        C(16.0, 14.2091, 14.2091, 16.0, 12.0, 16.0),
        C(9.7909, 16.0, 8.0, 14.2091, 8.0, 12.0),
        C(8.0, 9.7909, 9.7909, 8.0, 12.0, 8.0),
        C(14.2091, 8.0, 16.0, 9.7909, 16.0, 12.0),
        Z,
        M(12.0, 2.0),
        L(12.0, 4.0),
        M(12.0, 20.0),
        L(12.0, 22.0),
        M(4.93, 4.93),
        L(6.34, 6.34),
        M(17.66, 17.66),
        L(19.07, 19.07),
        M(2.0, 12.0),
        L(4.0, 12.0),
        M(20.0, 12.0),
        L(22.0, 12.0),
        M(6.34, 17.66),
        L(4.93, 19.07),
        M(19.07, 4.93),
        L(17.66, 6.34),
    ],
};

/// lucide keyboard
pub const KEYBOARD: Icon = Icon {
    grid: 24.0,
    fill: false,
    segs: &[
        M(10.0, 8.0),
        L(10.01, 8.0),
        M(12.0, 12.0),
        L(12.01, 12.0),
        M(14.0, 8.0),
        L(14.01, 8.0),
        M(16.0, 12.0),
        L(16.01, 12.0),
        M(18.0, 8.0),
        L(18.01, 8.0),
        M(6.0, 8.0),
        L(6.01, 8.0),
        M(7.0, 16.0),
        L(17.0, 16.0),
        M(8.0, 12.0),
        L(8.01, 12.0),
        M(4.0, 4.0),
        L(20.0, 4.0),
        C(21.1046, 4.0, 22.0, 4.8954, 22.0, 6.0),
        L(22.0, 18.0),
        C(22.0, 19.1046, 21.1046, 20.0, 20.0, 20.0),
        L(4.0, 20.0),
        C(2.8954, 20.0, 2.0, 19.1046, 2.0, 18.0),
        L(2.0, 6.0),
        C(2.0, 4.8954, 2.8954, 4.0, 4.0, 4.0),
        Z,
    ],
};

/// lucide lock
pub const LOCK: Icon = Icon {
    grid: 24.0,
    fill: false,
    segs: &[
        M(5.0, 11.0),
        L(19.0, 11.0),
        C(20.1046, 11.0, 21.0, 11.8954, 21.0, 13.0),
        L(21.0, 20.0),
        C(21.0, 21.1046, 20.1046, 22.0, 19.0, 22.0),
        L(5.0, 22.0),
        C(3.8954, 22.0, 3.0, 21.1046, 3.0, 20.0),
        L(3.0, 13.0),
        C(3.0, 11.8954, 3.8954, 11.0, 5.0, 11.0),
        Z,
        M(7.0, 11.0),
        L(7.0, 7.0),
        C(7.0, 4.2386, 9.2386, 2.0, 12.0, 2.0),
        C(14.7614, 2.0, 17.0, 4.2386, 17.0, 7.0),
        L(17.0, 11.0),
    ],
};

/// lucide log-out
pub const LOG_OUT: Icon = Icon {
    grid: 24.0,
    fill: false,
    segs: &[
        M(9.0, 21.0),
        L(5.0, 21.0),
        C(3.8954, 21.0, 3.0, 20.1046, 3.0, 19.0),
        L(3.0, 5.0),
        C(3.0, 3.8954, 3.8954, 3.0, 5.0, 3.0),
        L(9.0, 3.0),
        M(16.0, 17.0),
        L(21.0, 12.0),
        L(16.0, 7.0),
        M(21.0, 12.0),
        L(9.0, 12.0),
    ],
};

/// lucide chevron-right
pub const CHEVRON_RIGHT: Icon = Icon {
    grid: 24.0,
    fill: false,
    segs: &[
        M(9.0, 18.0),
        L(15.0, 12.0),
        L(9.0, 6.0),
    ],
};

/// lucide check
pub const CHECK: Icon = Icon {
    grid: 24.0,
    fill: false,
    segs: &[
        M(20.0, 6.0),
        L(9.0, 17.0),
        L(4.0, 12.0),
    ],
};

// ---- 图标库许可声明（原文） ----
//
// Lucide:
// ISC License
//
// Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2022.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
//
// Fluent UI System Icons:
// MIT License
//
// Copyright (c) 2020 Microsoft Corporation
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
