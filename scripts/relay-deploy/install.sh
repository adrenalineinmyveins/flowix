#!/usr/bin/env bash
# Flowix relay v2 一键安装（Linux + systemd）。
#
# 前置：拿到 Linux release 二进制，二选一——
#   a) 服务器上编译：git clone 本仓库 && cargo build --release -p flowix-relay
#   b) 交叉编译上传：cargo build --release -p flowix-relay --target x86_64-unknown-linux-gnu
#      （Windows 主机建议用 cargo-zigbuild）
#
# 用法（root，在仓库 scripts/relay-deploy/ 下执行）：
#   ./install.sh /path/to/flowix-relay
#
# 安装内容：
#   /opt/flowix-relay/flowix-relay      二进制
#   /var/lib/flowix-relay/relay.db      SQLite 状态（DynamicUser + StateDirectory 管理）
#   /etc/systemd/system/flowix-relay.service
# 中继监听 127.0.0.1:8788，TLS 由 Caddy/Nginx 反代提供（见 Caddyfile.relay）。
set -euo pipefail

BIN_SOURCE="${1:?用法: ./install.sh /path/to/flowix-relay}"
HERE="$(cd "$(dirname "$0")" && pwd)"

[[ -x "$BIN_SOURCE" ]] || { echo "找不到可执行文件: $BIN_SOURCE" >&2; exit 1; }

install -Dm755 "$BIN_SOURCE" /opt/flowix-relay/flowix-relay
install -Dm644 "$HERE/flowix-relay.service" /etc/systemd/system/flowix-relay.service
systemctl daemon-reload
systemctl enable --now flowix-relay

sleep 1
systemctl --no-pager --lines=5 status flowix-relay || true
echo
echo "完成。存活检查（任意 HTTP 响应码即进程在线）: curl -s -o /dev/null -w '%{http_code}\\n' http://127.0.0.1:8788/"
echo "公网暴露: 把 $HERE/Caddyfile.relay 的站点块并入 Caddy 配置（域名替换 relay.example.com）。"
