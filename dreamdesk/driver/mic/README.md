# DreamDesk 虚拟音频驱动（虚拟麦克风）

P2 第 2 项，方案和实测见 `dreamdesk/docs/P2-实施记录.md` 第 2 节。从原 DreamDesk 的 `dd_mic_driver` 原样搬来
（2026-10-22），只改了说明和注释。

⚠️ **签名**：现在用 WDK 测试证书签，只能在开着测试签名（`bcdedit /set testsigning on`）的机器上加载——
那是开发形态，不能推给终端。公司自有 CA 签的内核驱动在正常模式下也加载不了（Windows 的设计限制，原项目
`docs/内核驱动签名.md`）；正式发布走微软 attestation 签名（EV 证书 + 硬件开发者中心）。

一个设备「DreamDesk Virtual Audio」，两个端点：

- **麦克风**：被控端会话进程（RustDesk `--server`，SYSTEM）经 `\\.\DreamDeskMicCtl` 推单声道 S16 48 kHz PCM
  进驱动里的环，Windows 应用从端点录到它。声音来自 RustDesk 的语音通话（`src/ddpro/mic.rs`）。
- **扬声器**：渲染数据驱动直接丢掉。被控端没有真声卡时它是唯一的播放设备，RustDesk 才采集得到被控端的声音。
  不主动把它设成默认播放设备（原项目会话期间设，有真声卡的机器会被改掉设置）。

## 来源

`simpleaudio/` 是微软 [Windows-driver-samples](https://github.com/microsoft/Windows-driver-samples)
`audio/simpleaudiosample`（**MS-PL**），取自提交 `3c3fb49`（2026-09-18）。删掉正弦发生器
（ToneGenerator）、渲染落盘（SaveData）和只读它们的注册表项；麦克风格式改成单声道 S16 48 kHz、
四个处理模式；扬声器保持上游原样（48k/16/立体声）；端点名、ETW helper、控制设备接进 adapter。
和上游逐文件 `git diff --no-index` 即可看到全部改动（上游分 Main / Inc / Filters / Utilities
四个目录，文件名一致）。

```
simpleaudio/             微软样例（删减版）
src/dd_mic_ring.*        PCM 环 + 起播门限（抖动缓冲）
src/dd_mic_ctldev.*      控制设备 \\.\DreamDeskMicCtl（只允许 SYSTEM / 管理员）
include/dd_mic_ioctl.h   和用户态共用的 IOCTL 约定（src/ddpro/mic.rs 里照抄了写入的控制码）
inf/dreamdesk_mic.inf
test/sine_test.c         推 1 kHz 正弦或计数器（ramp）进驱动
test/wasapi_capture.c    从端点录 WAV；excl 模式绕过音频引擎，逐位可比
test/stats_query.c       读驱动的取空 / 溢出计数
```

## Build

```cmd
build.cmd
```

产物在 **`x64\Release\package\`**（`.sys` + `.inf` + `.cat`，都已签名）。
⚠️ **那个 `package\` 子目录是必须的**：`inf2cat /driver:` 会连子目录一起扫，
直接指 `x64\Release` 会挑中 `obj\` 里 MSBuild 生成的那份 inf 然后报 22.9.1。

`build.cmd` 用 vswhere **探测** MSBuild 和 Inf2Cat，不写死路径（不能用 VsDevCmd
包装，那个在非交互 shell 里不工作）。它还传 `/p:ApiValidator_Enable=false` ——
WDK 28000 的 `aitstatic.exe` 在开发机上返回 193，而 `.sys` 在那一步之前就编好
签好了。**正式发布时要把它打开。**

测试工具在 VS 的 x64 开发者命令行里编：

```cmd
cl /nologo /W3 /O2 test\sine_test.c winmm.lib
cl /nologo /W3 /O2 /std:c17 test\wasapi_capture.c ole32.lib uuid.lib oleaut32.lib
cl /nologo /W3 /O2 test\stats_query.c
```

## Install（开发）

```cmd
:: 一次性（要重启）:
bcdedit /set testsigning on
:: 之后在管理员命令行里、本目录下:
install.cmd
```

`install.cmd` 用 WDK 里的 devcon 建设备，只给开发机用。产品安装包的安装路径等正式签名的驱动到位后再做
（用 RustDesk 的 `win_device::install_driver`）。

**升级不用重启**：控制设备跟着音频适配器创建和删除，驱动能卸载，`pnputil` 当场换上新映像。

## 验证（驱动本身，逐位）

```cmd
sine_test.exe ramp            :: 一个窗口里推计数器
wasapi_capture.exe out.wav 5 excl
```

独占模式直接读驱动的缓冲，录到的应当是连续的 +1 计数器（到 65535 回绕到 1，跳过 0 好和静音区分）；
开头约 40 ms 的零是起播门限，接着 2 ms（96 个样本）是渐入。2026-10-22 在 209.32 上录 10 秒：除回绕外没有断点。
⚠️ **没有用户登录时，共享模式录到的整段是静音**（每个包都带 SILENT 标志）。推测是系统对无人登录 /
非交互会话的限制，未证实；验驱动用 excl。

## Uninstall

```cmd
install.cmd uninstall
```

⚠️ **装 Release，别装 Debug。** Debug 下 `ASSERT()` 是活的，断言一触发就执行
断点指令，没挂内核调试器直接 0x7E 蓝屏（2026-09-11 实撞）。`install.cmd` 默认装 Release。
