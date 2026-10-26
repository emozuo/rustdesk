#!/usr/bin/env bash
# DreamDesk Linux 打包：编出 dreamdesk-pro-<版本>.deb（产品改名、专业模式、只连内网服务器）。在仓库根目录跑：
#   bash dreamdesk/scripts/build-linux.sh [build.py 的额外开关，例如测试包的 --dd-managed-dev]
# 只出受管版：总是带 --dd-managed。正式包要先把产品钥填进 src/ddpro/managed.rs 的 PRODUCT_KEYS，没填时编译直接报错停下。
#
# 和 Windows 的 build-windows.ps1 同一个做法：编译走 RustDesk 自己的流程（build.py 的开关表、它 Linux 打包时用的同样的
# cargo / flutter 命令），只给程序文件和安装包改名，内部库 librustdesk.so 不动（Windows 也保留 librustdesk.dll），
# 其余名字（配置 / 日志 / IPC 目录、服务名、进程匹配、链接协议 dreamdesk-pro://、快捷键授权）由运行时应用名派生。
# 安装包里的服务、桌面文件、安装脚本从上游 res/ 的模板替换出来：上游改了模板，下次打包自动带上。
# 官方 Linux 发布流程没有改名机制（Windows 有 RDAPPNAM 模板），所以改名放在这里。
set -euo pipefail

APP=dreamdesk-pro # 必须和 src/ddpro/brand.rs 的 APP_NAME 一致
# 安装包的维护者一栏（dpkg 要求「名字 <邮箱>」的格式）
MAINTAINER="DreamDesk <dreamdesk@of3d.com>"

cd "$(dirname "$0")/../.."
grep -q "pub const APP_NAME: &str = \"$APP\";" src/ddpro/brand.rs || {
    echo "APP=$APP 和 src/ddpro/brand.rs 的 APP_NAME 不一致" >&2
    exit 1
}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# 安装包版本 = 程序版本 + 构建时间（如 1.5.1+202610231105）：程序版本跟着上游几个月才变一次，版本号不变的新构建
# apt 会当成「已是最新」跳过。按 Debian 的比较规则，带后缀的比不带的、比更早的构建都新；程序版本升了照常更新
DEB_VERSION="$VERSION+$(date -u +%Y%m%d%H%M)" # 用 UTC：换一台时区不同的机器打包，新包也不会比旧包「小」
case "$(uname -m)" in
    x86_64) FLUTTER_ARCH=x64 DEB_ARCH=amd64 ;;
    aarch64) FLUTTER_ARCH=arm64 DEB_ARCH=arm64 ;;
    *) echo "不支持的架构 $(uname -m)" >&2; exit 1 ;;
esac
# 开关只在 build.py 一处定（和官方 Linux 发布版一样带 hwcodec）
FEATURES=$(python3 build.py --flutter --hwcodec --dd-pro --dd-brand --dd-managed "$@" --print-features)
# 测试包另加 build.py 没有的开关（正式包不要设）
[ -n "${EXTRA_FEATURES:-}" ] && FEATURES="$FEATURES,$EXTRA_FEATURES"
echo "## $APP $DEB_VERSION ($DEB_ARCH), features: $FEATURES"

echo "## pub get + bridge codegen"
(cd flutter && flutter pub get >/dev/null)
flutter_rust_bridge_codegen --rust-input ./src/flutter_ffi.rs --dart-output ./flutter/lib/generated_bridge.dart \
    --c-output ./flutter/macos/Runner/bridge_generated.h >/dev/null

echo "## cargo"
cargo build --locked --features "$FEATURES" --lib --release
# build.py 编完 Rust 做的同一处修正（ffi_bindgen_function_refactor）
sed -i "s/ffi.NativeFunction<ffi.Bool Function(DartPort/ffi.NativeFunction<ffi.Uint8 Function(DartPort/g" \
    flutter/lib/generated_bridge.dart

echo "## flutter"
# 程序图标（dreamdesk/res，gen-icons.py 生成）：flutter/assets/icon.png 是 RustDesk 给定制客户端的入口（托盘、界面里的小图标），
# 仓库里原本没有（不开 DreamDesk 开关编的程序要和原版一样），编之前放进去、打完删掉。要在 flutter build 之前放：
# 编完才放的话不在资源清单里，界面取不到
cleanup() { rm -f flutter/assets/icon.png; [ -z "${T:-}" ] || rm -rf "$T"; }
trap cleanup EXIT
cp dreamdesk/res/icon-256.png flutter/assets/icon.png
(cd flutter && flutter build linux --release)
BUNDLE=flutter/build/linux/$FLUTTER_ARCH/release/bundle

echo "## 打包"
# 模板里要换的只有名字（路径、程序名、服务名、图标名、协议名、显示名）。网址之类出现就停下，免得被一起换掉
TEMPLATES="res/rustdesk.service res/rustdesk.desktop res/rustdesk-link.desktop res/DEBIAN/postinst res/DEBIAN/prerm res/DEBIAN/preinst res/DEBIAN/postrm"
if grep -n -i -E 'rustdesk\.(com|org|net)|github' $TEMPLATES; then
    echo "上面这些模板行有网址，替换规则要先按行区分" >&2
    exit 1
fi
# 顺带去掉行尾回车：工作区从 Windows 拷过来时文件带 CRLF，桌面文件、服务文件、安装脚本里留着回车会出问题
rename() { sed -e 's/\r$//' -e "s/RustDesk/$APP/g" -e "s/rustdesk/$APP/g" "$1" >"$2"; }

T=$(mktemp -d) # 由上面的 cleanup 删
D=$T/deb
mkdir -p "$D/usr/bin" "$D/usr/share/$APP/files/systemd" "$D/usr/share/icons/hicolor/256x256/apps" \
    "$D/usr/share/icons/hicolor/scalable/apps" "$D/usr/share/applications" "$D/usr/share/polkit-1/actions" "$D/DEBIAN"
cp -r "$BUNDLE"/* "$D/usr/share/$APP/"
mv "$D/usr/share/$APP/rustdesk" "$D/usr/share/$APP/$APP"
rename res/rustdesk.service "$D/usr/share/$APP/files/systemd/$APP.service"
cp dreamdesk/res/icon-256.png "$D/usr/share/icons/hicolor/256x256/apps/$APP.png"
cp dreamdesk/res/icon.svg "$D/usr/share/icons/hicolor/scalable/apps/$APP.svg"
rename res/rustdesk.desktop "$D/usr/share/applications/$APP.desktop"
rename res/rustdesk-link.desktop "$D/usr/share/applications/$APP-link.desktop"
printf '#!/bin/sh\n' >"$D/usr/share/$APP/files/polkit"
chmod a+x "$D/usr/share/$APP/files/polkit"
for s in postinst prerm preinst postrm; do
    rename "res/DEBIAN/$s" "$D/DEBIAN/$s"
    chmod 755 "$D/DEBIAN/$s"
done
# 专业画面通道的 UDP 接收缓冲：通道申请 8 MB（静止无损整屏补发一次约 830 KB 突发），系统默认上限只有约 200 KB，
# 会被截成约 400 KB——实测每次整屏补发在本机缓冲里丢几百个包、全靠补发救回（P1 实施记录「审查：2026-10-21」后的实测）。
# 放宽上限是低延迟 UDP 传输的标准部署做法（QUIC、WebRTC、SRT 同样要求）；只放宽「每个程序最多能要多少」，不多占内存。
# 放在 /usr/lib/sysctl.d（包自己的文件，卸载时一起删）；编号 60，管理员自己的配置（通常 99-）能覆盖它
mkdir -p "$D/usr/lib/sysctl.d"
printf '# %s：专业画面通道的 UDP 接收缓冲上限（通道申请 8 MB）\nnet.core.rmem_max = 8388608\n' "$APP" \
    >"$D/usr/lib/sysctl.d/60-$APP.conf"
# 装好就生效（不用等重启）；当前上限已经更大时不往小改
cat >>"$D/DEBIAN/postinst" <<'EOF'

if [ "$1" = configure ] && [ "$(sysctl -n net.core.rmem_max 2>/dev/null || echo 0)" -lt 8388608 ]; then
	sysctl -q -w net.core.rmem_max=8388608 || true
fi
EOF
# 依赖照 build.py 的 control 模板（同一份列表）；和 rustdesk 包互斥、可替换它（用户 2026-10-20 定：两者的服务抢同一个端口，
# 本来就不能同时被连；装 dreamdesk-pro 时 apt 自动卸掉 rustdesk）
DEPENDS=$(sed -n 's/^Depends: \(.*\)%s\r\{0,1\}$/\1/p' build.py)
[ -n "$DEPENDS" ] || { echo "没从 build.py 读到依赖列表" >&2; exit 1; }
cat >"$D/DEBIAN/control" <<EOF
Package: $APP
Section: net
Priority: optional
Version: $DEB_VERSION
Architecture: $DEB_ARCH
Maintainer: $MAINTAINER
Depends: $DEPENDS
Recommends: libayatana-appindicator3-1
Conflicts: rustdesk
Replaces: rustdesk
Description: DreamDesk remote desktop (pro mode), based on RustDesk.

EOF
(cd "$D" && find usr -type f -exec md5sum {} + >DEBIAN/md5sums)
# 先打在临时目录、自检过了才挪到仓库根：查出问题时不能已经把上一个好的包覆盖掉
OUT=$T/$APP-$DEB_VERSION.deb
dpkg-deb --root-owner-group -b "$D" "$OUT" >/dev/null

echo "## 自检"
# 包里不许有名字带 rustdesk 的程序、服务、桌面文件、图标（内部库 librustdesk.so 放过），装好的文件内容里也不许残留
LEFT=$(dpkg-deb -c "$OUT" | awk '{print $NF}' | grep -i rustdesk | grep -v "/lib/librustdesk\.so$" || true)
[ -z "$LEFT" ] || { echo "包里还有 rustdesk 的名字："; echo "$LEFT"; exit 1; } >&2
[ "$(dpkg-deb -f "$OUT" Package)" = "$APP" ] || { echo "包名不是 $APP" >&2; exit 1; }
# control 里的「和 rustdesk 互斥、可替换」「基于 RustDesk」是有意写的，不查
if grep -rn -i rustdesk "$D"/DEBIAN/{postinst,prerm,preinst,postrm} \
    "$D/usr/share/applications" "$D/usr/share/$APP/files"; then
    echo "上面这些安装文件里还有 rustdesk" >&2
    exit 1
fi
# 文件名带构建时间、每次都不同：仓库根只留这一个，之前打的包删掉
rm -f "./$APP"-*.deb
mv "$OUT" .
echo "OK: $(basename "$OUT")"
