#!/usr/bin/env bash
# DreamDesk kiosk 安装包：dreamdesk-kiosk-<版本>.deb（不含程序，依赖 dreamdesk-pro）。在仓库根目录跑：
#   bash dreamdesk/scripts/build-kiosk-deb.sh
# 装在全新的 Ubuntu 26.04 上：建 kiosk 账户、开机自动登录进只跑 dreamdesk-pro --kiosk 的 GNOME Kiosk 会话；
# 卸载还原自动登录设置，彻底删除（purge）连账户一起删。方案见 dreamdesk/docs/K-kiosk方案.md
set -euo pipefail

APP=dreamdesk-pro
PKG=dreamdesk-kiosk
MAINTAINER="DreamDesk <dreamdesk@of3d.com>"

cd "$(dirname "$0")/../.."
VERSION=$(sed -n 's/^version = "\(.*\)"\r\{0,1\}$/\1/p' Cargo.toml | head -1)
DEB_VERSION="$VERSION+$(date -u +%Y%m%d%H%M)" # 和 build-linux.sh 同一个版本规则
SRC=dreamdesk/kiosk

T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
D=$T/deb
mkdir -p "$D/DEBIAN" "$D/usr/share/wayland-sessions" "$D/usr/share/gnome-session/sessions" \
    "$D/usr/lib/systemd/user/gnome-session@$PKG.target.d"
# 去掉行尾回车：工作区从 Windows 拷过来时文件带 CRLF
crlf() { sed -e 's/\r$//' "$1" >"$2"; }
crlf "$SRC/wayland-sessions/$PKG.desktop" "$D/usr/share/wayland-sessions/$PKG.desktop"
crlf "$SRC/gnome-session/$PKG.session" "$D/usr/share/gnome-session/sessions/$PKG.session"
crlf "$SRC/systemd-user/$PKG.service" "$D/usr/lib/systemd/user/$PKG.service"
crlf "$SRC/systemd-user/gnome-session@$PKG.target.d/session.conf" \
    "$D/usr/lib/systemd/user/gnome-session@$PKG.target.d/session.conf"
for s in postinst postrm; do
    crlf "$SRC/DEBIAN/$s" "$D/DEBIAN/$s"
    chmod 755 "$D/DEBIAN/$s"
done
cat >"$D/DEBIAN/control" <<EOF
Package: $PKG
Section: net
Priority: optional
Version: $DEB_VERSION
Architecture: all
Maintainer: $MAINTAINER
Depends: $APP, gnome-kiosk, gdm3, accountsservice, openssh-server
Description: DreamDesk kiosk: boots straight into a full-screen connection to the bound workstation.

EOF
(cd "$D" && find usr -type f -exec md5sum {} + >DEBIAN/md5sums)
OUT=$T/$PKG-$DEB_VERSION.deb
dpkg-deb --root-owner-group -b "$D" "$OUT" >/dev/null
rm -f "./$PKG"-*.deb
mv "$OUT" .
echo "OK: $(basename "$OUT")"
