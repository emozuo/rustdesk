# RustDesk 构建与部署

本机验证环境：Windows 11 + VS 2026 Community 18.7.1 + Rust stable 1.98.1 + Flutter 3.24.5 +
CMake 4.3.2 + vcpkg（baseline `9e593bb1`），Linux 侧为 WSL Ubuntu 24.04（以 root 构建）。

下面所有命令都在本机实跑过，产物：

| 产物 | 大小 | 实测 |
|---|---|---|
| `rustdesk-1.5.0-install.exe` | 22.8 MB | `rustdesk.exe --version` → 1.5.0 |
| `rustdesk-1.5.0.deb` | 24 MB | `./rustdesk --version` → 1.5.0 |
| `hbbs` / `hbbr` / `rustdesk-utils` | 6.3 / 2.6 / 0.4 MB | 端口全部监听正常，见 §4 |

---

## 0. 前置：本机网络（国内必做）

`aomedia.googlesource.com` / `chromium.googlesource.com` 不可达，会卡住 vcpkg 的 aom、libyuv
和 cargo 依赖 `rust-webm` 的 libwebm 子模块。

**Windows —— 走本机已有的 SOCKS5 代理：**

```powershell
$env:ALL_PROXY = "socks5h://127.0.0.1:10808"
```

**WSL —— NAT 模式下访问不到 Windows 的 127.0.0.1，改用本地裸仓库镜像：**

```powershell
# 在 Windows 上带代理做一次镜像（只需一次）
$env:ALL_PROXY = "socks5h://127.0.0.1:10808"
git clone --mirror https://aomedia.googlesource.com/aom          D:\WORKPLACE\CODING\_toolchain\mirrors\aom.git
git clone --mirror https://chromium.googlesource.com/libyuv/libyuv D:\WORKPLACE\CODING\_toolchain\mirrors\libyuv.git
git clone --mirror https://chromium.googlesource.com/webm/libwebm  D:\WORKPLACE\CODING\_toolchain\mirrors\libwebm.git
# vcpkg 按 SHA 拉取，裸仓库需放开
Get-ChildItem D:\WORKPLACE\CODING\_toolchain\mirrors -Directory | ForEach-Object {
  git -C $_.FullName config uploadpack.allowAnySHA1InWant true
  git -C $_.FullName config uploadpack.allowReachableSHA1InWant true
}
```

```bash
# WSL 内把被墙的 URL 指向本地镜像
MIRR=/mnt/d/WORKPLACE/CODING/_toolchain/mirrors
git config --global url."$MIRR/aom.git".insteadOf     "https://aomedia.googlesource.com/aom"
git config --global url."$MIRR/libyuv.git".insteadOf  "https://chromium.googlesource.com/libyuv/libyuv"
git config --global url."$MIRR/libwebm.git".insteadOf "https://chromium.googlesource.com/webm/libwebm"
# 镜像在 /mnt/d 上，属主在 WSL 里对不上，不加例外 git 会以 dubious ownership 拒绝访问
for r in aom libyuv libwebm; do git config --global --add safe.directory "$MIRR/$r.git"; done
```

---

## 1. 服务端 hbbs / hbbr

不在客户端仓库里，是独立仓库。

```bash
git clone https://github.com/rustdesk/rustdesk-server.git
cd rustdesk-server
git submodule update --init --recursive
cargo build --release
# 产物：target/release/{hbbs,hbbr,rustdesk-utils}
```

---

## 2. Windows 客户端

```powershell
# 依赖
rustup toolchain install stable
rustup component add rustfmt              # codegen 要用 rustfmt，minimal profile 没有
cargo install flutter_rust_bridge_codegen --version 1.80.1 --features uuid --locked
# Flutter 3.24.5（版本必须对齐，项目 pin 了 pubspec.lock）
#   下载 flutter_windows_3.24.5-stable.zip 解压后打补丁：
git -C <flutter-sdk> apply <rustdesk>\.github\patches\flutter_3.24.4_dropdown_menu_enableFilter.diff
# vcpkg：commit 和 vcpkg.json 的 baseline 对齐（同 Linux 侧）。本机 2026-10-21 起用单独检出的
#   D:\WORKPLACE\CODING\_toolchain\vcpkg-rustdesk（从 C:\vcpkg 建的 git worktree，C:\vcpkg 停在 5 月的提交不动）：
#   git -C C:\vcpkg fetch origin
#   git -C C:\vcpkg worktree add --detach D:\WORKPLACE\CODING\_toolchain\vcpkg-rustdesk 9e593bb18ea69cc5095e012465dcd675a822ed0d
#   powershell -File D:\WORKPLACE\CODING\_toolchain\vcpkg-rustdesk\scripts\bootstrap.ps1 -disableMetrics
# 然后在仓库根执行。宿主三元组必须和目标一样是静态的（官方流程 VCPKG_DEFAULT_HOST_TRIPLET 同样这么设）：
# FFmpeg 在清单里是「宿主」包，NVIDIA / AMD / Intel 硬件编码只在静态三元组里开，默认的动态宿主编出来的 FFmpeg 没有硬件编码
$env:VCPKG_DEFAULT_HOST_TRIPLET = "x64-windows-static"
vcpkg install --triplet x64-windows-static --x-install-root="$env:VCPKG_ROOT\installed"
# vcpkg 换了版本（库变了）之后，cargo 不会自己重新链接：先 cargo clean --release -p hwcodec -p scrap -p magnum-opus
```

**装的是 VS 2026（major 18）时还要补一处**。Flutter 3.24.5 能认出 VS 2026，但「VS 主版本 →
CMake 生成器」的映射表只写到 17，落到默认的 `Visual Studio 16 2019` 就会报
`could not find any instance of Visual Studio`。CMake 4.1+ 本身是支持 `Visual Studio 18 2026` 的。

```
# <flutter-sdk>\packages\flutter_tools\lib\src\windows\visual_studio.dart
  String? get cmakeGenerator {
    return switch (_majorVersion) {
+     18 => 'Visual Studio 18 2026',
      17 => 'Visual Studio 17 2022',
      _  => 'Visual Studio 16 2019',
    };
```

```powershell
# 改完删掉 stamp，强制 flutter_tools 重新编译 snapshot，否则改动不生效
Remove-Item <flutter-sdk>\bin\cache\flutter_tools.stamp
```

用 VS 2022 则无需这一步。

```powershell
# 环境
$env:PATH = "<flutter-sdk>\bin;C:\Python312;C:\Python312\Scripts;$env:USERPROFILE\.cargo\bin;" + $env:PATH
$env:ALL_PROXY = "socks5h://127.0.0.1:10808"
$env:VCPKG_ROOT = "D:\WORKPLACE\CODING\_toolchain\vcpkg-rustdesk"   # 2026-10-21 前是 C:\vcpkg（FFmpeg 7.1，旧）
Copy-Item C:\Python312\python.exe C:\Python312\python3.exe   # build.py 调的是 python3

cd <rustdesk>
git submodule update --init --recursive

# 顺序不能反：codegen 的 ffigen 要读 flutter/.dart_tool/package_config.json
cd flutter; flutter pub get; cd ..

flutter_rust_bridge_codegen --rust-input ./src/flutter_ffi.rs `
  --dart-output ./flutter/lib/generated_bridge.dart `
  --c-output ./flutter/macos/Runner/bridge_generated.h

python3 build.py --flutter
# 产物：rustdesk-1.5.0-install.exe（仓库根，自解压安装包）
```

换过 VS 版本 / 改过上面那张生成器映射表之后，必须先删掉旧的 CMake 缓存，否则报
`generator ... does not match the generator used previously`：

```powershell
Remove-Item -Recurse -Force flutter\build\windows
```

构建会在工作树里留下两样东西，提交前记得清：`flutter pub get` 会改写
`flutter/pubspec.lock`（3.24.5 会把几个传递依赖降级），portable 打包器会生成
`libs/portable/app_metadata.toml`（`data.bin` 已被 gitignore）。

---

## 3. Linux 客户端（WSL Ubuntu 24.04）

```bash
# 系统依赖
apt-get update && apt-get install -y \
  zip unzip g++ gcc git curl wget nasm yasm libgtk-3-dev clang cmake make ninja-build \
  libxcb-randr0-dev libxdo-dev libxfixes-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libasound2-dev libpulse-dev libclang-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  pkg-config libappindicator3-dev libvdpau-dev libva-dev libdrm-dev python3 \
  autoconf automake libtool-bin autoconf-archive
# 最后一行是 vcpkg 编 mfx-dispatch 要的；注意是 libtool-bin 不是 libtool，
# 后者只给 libtoolize，vcpkg 找的是 libtool 这个可执行文件。

# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env" && rustup component add rustfmt

# vcpkg（commit 与 vcpkg.json 的 baseline 对齐）
git clone https://github.com/microsoft/vcpkg.git ~/vcpkg
git -C ~/vcpkg checkout 9e593bb18ea69cc5095e012465dcd675a822ed0d
~/vcpkg/bootstrap-vcpkg.sh -disableMetrics
export VCPKG_ROOT=$HOME/vcpkg

# Flutter 3.24.5
curl -fL -o flutter.tar.xz https://storage.googleapis.com/flutter_infra_release/releases/stable/linux/flutter_linux_3.24.5-stable.tar.xz
tar xf flutter.tar.xz && export PATH="$HOME/flutter/bin:$PATH"
git -C ~/flutter apply ~/rustdesk/.github/patches/flutter_3.24.4_dropdown_menu_enableFilter.diff

# 源码（放 WSL 原生盘，不要放 /mnt/d，IO 差很多）
git clone https://github.com/rustdesk/rustdesk.git ~/rustdesk
cd ~/rustdesk && git submodule update --init --recursive
```

```bash
cd ~/rustdesk
$VCPKG_ROOT/vcpkg install --triplet x64-linux --x-install-root="$VCPKG_ROOT/installed"

cargo install flutter_rust_bridge_codegen --version 1.80.1 --features uuid --locked
cd flutter && flutter pub get && cd ..
flutter_rust_bridge_codegen --rust-input ./src/flutter_ffi.rs --dart-output ./flutter/lib/generated_bridge.dart

python3 build.py --flutter
# 产物：rustdesk-1.5.0.deb
```

> **在 Ubuntu 24.04 上构建 = 产物只能装 Ubuntu 24.04+。** 实测符号版本：
> `librustdesk.so` 要 `GLIBC_2.39`（正好是 24.04 的 glibc），启动器 `rustdesk` 要 `GLIBCXX_3.4.32`（GCC 13）。
> 依赖里还有 `libgtk-3-0t64`、`libasound2t64` 这些 24.04 的 t64 包名（control 里带
> `| libgtk-3-0` 备选，但 glibc 那条卡死）。
> 官方 CI 是在 ubuntu18.04 容器里编的，兼容面大得多；要发给老系统就得换低版本容器编。

---

## 4. 服务端部署

端口（实测）：

| 进程 | 端口 | 用途 |
|---|---|---|
| hbbs | 21115/tcp | NAT 类型探测 |
| hbbs | 21116/tcp + 21116/udp | ID 注册、心跳、打洞 |
| hbbs | 21118/tcp | WebSocket |
| hbbr | 21117/tcp | 中继 |
| hbbr | 21119/tcp | WebSocket 中继 |

```bash
# 安装
install -m755 target/release/hbbs target/release/hbbr /usr/bin/
mkdir -p /var/lib/rustdesk-server /var/log/rustdesk-server
cp systemd/rustdesk-hbbs.service systemd/rustdesk-hbbr.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now rustdesk-hbbr rustdesk-hbbs

# 首次启动会在工作目录生成 id_ed25519 / id_ed25519.pub，客户端要填的 Key 就是 .pub 内容
cat /var/lib/rustdesk-server/id_ed25519.pub

# 放行端口
ufw allow 21115:21119/tcp && ufw allow 21116/udp
```

```bash
# hbbs 和 hbbr 不同机、或端口非标准时才需要
hbbs -r <relay-host>:21117
```

---

## 5. 客户端指向自建服务器

三个值：ID 服务器 = hbbs 地址，中继服务器留空（同机同端口时客户端自己推导），Key = `id_ed25519.pub` 内容。

```powershell
# 命令行改（需已安装 + 管理员）
rustdesk --option custom-rendezvous-server 192.168.1.10
rustdesk --option key "<id_ed25519.pub 的内容>"
rustdesk --option relay-server 192.168.1.10     # 仅当中继不在 hbbs 同机/同端口
```

也可在界面「设置 → 网络」里填同样三项（ID 服务器 / 中继服务器 / Key）。

---

## 6. 内网直连

两种，按是否要 ID 服务器区分。

**A. 有 hbbs，走打洞直连（默认就是）**

两端注册到同一台 hbbs 即可；同网段打洞必成，数据不经 hbbr。确认 hbbs 日志里是
`ALWAYS_USE_RELAY=N`（默认值），不要设成 `Y`。

**B. 完全不要服务器，直接按 IP 连**

```powershell
# 被控端：开启直接 IP 访问
rustdesk --option direct-server Y
rustdesk --option direct-access-port 21118    # 可选，默认就是 21118
```

主控端在 ID 框直接填地址即可：

```
192.168.1.100
192.168.1.100:21118
```

界面对应项：被控端「设置 → 安全 → 允许 IP 直接访问」，勾上后下面会出现端口输入框。

> 客户端填裸 IP 时拨的是 `RELAY_PORT+1 = 21118`，与被控端 `direct-access-port`
> 默认值 `RENDEZVOUS_PORT+2 = 21118` 一致；改了端口就要用 `IP:端口` 形式连。
