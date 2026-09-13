# 根任务入口：把 Rust 与 web 两套工具链编排成统一命令（命令口径与 CI 一致）。
# Unix 走 sh；Windows 在无 sh 环境下回退 PowerShell（just 的 windows-shell），
# 因此每条命令独立成行、不用 &&，两种 shell 行为一致。
set shell := ["sh", "-cu"]
set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

# 列出全部任务
default:
    @just --list

# 安装 git hooks（core.hooksPath → .githooks；也可 sh scripts/install-hooks.sh）
hooks:
    git config core.hooksPath .githooks
    @echo "git hooks 已挂载到 .githooks/（pre-commit / commit-msg / pre-push）"

# 快速检查（pre-commit 同款，不分流全量跑；oxlint ~17ms / oxfmt ~3ms）
lint:
    cargo fmt --all --check
    pnpm -C web lint
    pnpm -C web format:check

# 完整门禁（检查集合与 CI 同口径；顺序相反——CI 先跑快的 web，本地先
# cargo 后 web 属习惯取舍。--timings 与 pnpm install 是 CI 专属，本地假设
# 已在 web/ 执行过 pnpm install；推送前或手动跑）
gate:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    pnpm -C web sync:sprites
    pnpm -C web lint
    pnpm -C web format:check
    pnpm -C web typecheck
    pnpm -C web build

# 开发模式：构建宿主后启动 Tauri 桌面壳（vite dev 于 :5180）
dev:
    pnpm -C web app:dev
