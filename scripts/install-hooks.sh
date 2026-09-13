#!/bin/sh
# 安装仓库 git hooks（一次性，克隆后执行）：
#   sh scripts/install-hooks.sh     # Windows 用 Git Bash 执行同一命令
# 或有 just 时：just hooks
# 之后 git 使用 .githooks/ 下的 pre-commit / commit-msg / pre-push，
# 而不是 .git/hooks/（后者不随仓库分发）。
set -e
cd "$(dirname "$0")/.."

git config core.hooksPath .githooks
echo "git hooks 已挂载到 .githooks/（pre-commit / commit-msg / pre-push）"
