# CLAUDE.md

本文件为 Claude Code (claude.ai/code) 在本仓库中工作时提供指引。

**对话一律用中文**：汇报、进度说明、工具调用附带的说明文字都用中文。代码、日志、命令、仓库里的英文规则文件
都不改变这一条。

@dreamdesk/docs/AGENTS-中文.md

上面导入的是仓库根目录 `AGENTS.md` 的中文译本。`AGENTS.md` 是共享规则手册，
由上游维护 —— 目录结构、Rust / Tokio 规则、编辑卫生、PR 评审与本地化规则都归它管，**以英文原文为准**。
本文件不重复那些内容，只补它没写的部分。

导入译本而不是英文原文，是为了让对话保持中文（大段英文规则会把回复带成英文）。译本开头记着它译自哪个上游提交；
**每次合并上游后**，用 `git log -1 --format=%h -- AGENTS.md` 对一下，原文变了就同步更新译本。

## 产品定位（用户 2026-10-27 定）

`dreamdesk` 分支就是 DreamDesk 产品，**只有受管版**（`dd-managed`）：必须登录、只连网关授权的受管工作站、不做设备号 / 口令连接。
设计、取舍、测试、验收一律只按受管版考虑。代码里 `kDdManaged` 为假 / 不开 `dd-managed` 的那条路只是 RustDesk 原样代码，
留着是为了每月合并上游少冲突，不当产品维护；但改动仍按下面上游规则写成薄钩子，不去改写它。

## 本地化（本分支的规定，优先于上面「本地化」一节）

产品只维护**简体中文和英文**（用户 2026-10-19 定）。我们新加的词条**只追加到 `src/lang/cn.rs`**——英文就是键本身，
不用另加；其它语言文件、`template.rs` 一律不动（缺条目时自动回落英文），免得每月合并上游时几十个文件冲突。
上游自己的词条随合并照常进来，不删其它语言文件。产品版（`dd-brand`）设置页的语言列表只给简体中文和英文，
按系统语言自动选时中文（含繁体）用简体中文、其余用英文（`src/ddpro/brand.rs`）。

## 构建

前置条件：C++ 工具链，以及设置好 `VCPKG_ROOT` 的 vcpkg。`vcpkg.json` 是 manifest ——
在仓库根目录执行 `vcpkg install` 会自动使用其中锁定的 baseline、`res/vcpkg` 里的 overlay
ports 和 `res/vcpkg-triplets` 里的 overlay triplets。`libs/hbb_common` 是 git submodule，
新克隆需要 `git submodule update --init --recursive`。

两套 UI 共用同一个二进制，由 `flutter` cargo feature 切换：

```sh
# Sciter（已废弃，默认 features）。需要把 sciter 动态库放在二进制旁边，
# 下载链接见 README。
cargo run

# Flutter。FFI bridge 是生成的且被 .gitignore，所以 codegen 必须在
# 第一次 cargo build 之前跑，否则 `mod bridge_generated;` 解析不到。
cargo install flutter_rust_bridge_codegen --version 1.80.1 --features uuid --locked
flutter_rust_bridge_codegen --rust-input ./src/flutter_ffi.rs \
  --dart-output ./flutter/lib/generated_bridge.dart \
  --c-output ./flutter/macos/Runner/bridge_generated.h
cargo build --locked --features flutter
cd flutter && flutter run     # 或者 ./run.sh，它把上面这些都做了
```

每次改动 `src/flutter_ffi.rs` 后都要重跑 codegen（包括合并上游带进来的改动）。上游 `AGENTS.md` 写的「不要跑 codegen、
手写 Dart 封装」是给上游维护者的环境说的（那边生成的桥接文件由 CI 产出）；本分支本地打包要用生成的桥接文件，照旧用上面
锁定的 1.80.1 跑。

`build.py` 是打包驱动（deb/rpm/dmg/pkg/portable），不是开发循环。它在唯一一处把命令行 flag
映射成 cargo features；`python3 build.py --flutter --hwcodec --print-features` 只打印它会用的
feature 列表而不构建，这样自己手写的 cargo 命令行就不会和它漂移。其它 flag：`--vram`、
`--drm`、`--unix-file-copy-paste`、`--portable`、`--skip-cargo`，以及 DreamDesk 的 `--dd-pro`、
`--dd-brand`（Windows 带改名的打包走 `dreamdesk/scripts/build-windows.ps1`，直接用 build.py 会被拒绝）。

feature 说明：`hwcodec` / `vram` / `mediacodec` / `drm` 都转发给 `scrap`。`drm-wake` 只在 `drm`
之上额外控制"注入合成指针事件唤醒显示器"这一段。release profile 是 `lto + codegen-units=1 +
panic=abort`，所以 release 构建很慢 —— 迭代时用 debug。

## 测试

CI 不跑 Rust 测试，但代码里有大量 `#[cfg(test)]` 模块，值得在本地跑。

```sh
cargo test --lib                                 # librustdesk 的单元测试
cargo test --lib server::connection::tests       # 只跑一个模块
cargo test --lib -- --exact <full::test::path>   # 只跑一个测试
cargo test -p base -p hbb_common                 # 单个 workspace 成员

cd flutter && flutter test                            # 全部 Dart 测试
flutter test test/terminal_mouse_handler_test.dart    # 单个文件
```

Rust 测试就放在被测代码旁边、属于 `librustdesk` 这个 lib crate，所以加 `--lib` 可以避免连带
构建 `naming` / `service` 两个二进制。

## 架构

### 一个二进制，多种角色

`src/core_main.rs` 按 argv 分发，是进程决定"自己是谁"的唯一入口。它返回 `Some(args)` 表示把
控制权交给 UI，返回 `None` 表示该角色已经自己跑完了。角色：无参数 → UI；`--server` → 常驻的
宿主进程（整个 `src/server/` 加 `rendezvous_mediator`）；`--cm` → 处理入站会话的连接管理器
窗口；`--tray`；`--service` → 监管 `--server` 的平台系统服务；`--elevate` / `--run-as-system` /
`--portable-service` → 提权辅助进程；`--connect` / `--file-transfer` / `--port-forward` /
`--terminal` / `--view-camera` / `--rdp` → 出站会话窗口。新增角色是在这里加一个分支，
不是加一个新二进制。

入口点因 UI 而异。Sciter：`src/main.rs` 直接调 `core_main()`。Flutter：Flutter runner
（`flutter/windows/runner/main.cpp`、`flutter/linux/main.cc`）先 `dlopen` `librustdesk`，调用导出的
`rustdesk_core_main` / `rustdesk_core_main_args`（`src/flutter.rs`），然后才启动 Dart 引擎 ——
所以在 `--features flutter` 下 `src/main.rs` 只是个桩，不是真正的入口。

### 进程间 IPC

UI、`--cm`、`--server` 是彼此独立的进程，通过 `parity-tokio-ipc` 的命名管道 / unix socket 通信。
`src/ipc.rs` 管这一层：`ipc::Data` 是消息枚举，`new_listener(postfix)` 开一条通道，postfix 决定
是哪条（`""` = Windows 主通道，`POSTFIX_SERVICE` = 特权服务通道，`"_cm"` = 连接管理器，
`"_pa"` = pulseaudio 辅助进程）。`_service` 通道对每个连接做授权校验、并限制能接受哪些消息；
往它上面加消息属于安全相关改动，不是普通管道铺设。

### 连接生命周期

`src/rendezvous_mediator.rs` 维持与 rustdesk-server (hbbs) 的注册，并处理打洞 / 中继请求；
接受的连接落到 `src/server/connection.rs`，每个对端一个 `Connection`，权限模型和协议消息
处理也都在这里。出站连接从 `src/client.rs`（`Client`）发起，消息循环在
`src/client/io_loop.rs`。

### 服务与订阅者

`src/server/service.rs` 定义了各媒体服务共用的 `Service` / `Subscriber` 模式：每个服务
（`audio`、`display`、video、`mouse_cursor`、`mouse_pos`、`window_focus`、`remote-printer`、
剪贴板、终端）跑自己的循环，并向订阅它的 `Connection` 广播。第一个订阅者到来时服务启动，
最后一个离开时停止 —— 采集开销随需求走。服务名是各模块里的 `pub const NAME`。

### UI 抽象

`src/ui_session_interface.rs` 是"一套客户端实现同时服务两套 UI"的关键：`Session<T>` 持有连接
状态并实现 `Interface`，而 `T: InvokeUiSession` 是 UI 那一侧。Sciter 和 `FlutterHandler`
（`src/flutter.rs`）是两个实现。连接管理器和主窗口对应 `src/ui_cm_interface.rs` /
`src/ui_interface.rs`。新的客户端行为应该写进 `Session`，而不是写进某一套 UI。

Flutter 侧的事件是 JSON，经由 flutter_rust_bridge 的 stream sink 送达 —— `src/flutter.rs` 里的
`push_event`（按会话）和 `push_global_event`（按通道）；Dart 侧在 `flutter/lib/models/` 消费。
视频帧不走这条通道：它走注册进来的纹理指针（`session_register_pixelbuffer_texture`、
`session_register_gpu_texture`），或者由 Dart 侧主动拉取的 RGBA 缓冲
（`session_get_rgba_size` / `session_next_rgba`）。

### 配置与选项

所有 option key 都走 `base::config::keys`（`libs/base/src/config/keys.rs`）—— 整套 key（包括
`hbb_common` 仍在读的那几个）只有这一条 import 路径。不要在别处另立一份 key 常量。
