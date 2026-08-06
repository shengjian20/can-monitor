# Contributing

欢迎参与 can-monitor 的开发。项目很小但门禁严格, 改动前先读这几条。本文件是公开仓库的协作规范入口; 版本发布流程见 [docs/release-process.md](docs/release-process.md), 变更历史见 [CHANGELOG.md](CHANGELOG.md)。

## 环境

- Rust stable (建议 ≥ 1.88), workspace 根目录 `cargo build` 应零错误
- Linux 需要 CAN 工具链做 vcan 联调 (可选, `scripts/vcan-setup.sh`)
- GUI 形态需要 webkit2gtk / dbus 系统依赖 (见 README), 本机装不上时用 CI 验证
- 仓库自带 devcontainer, 包含全部工具链 (rustc / zig / can-utils / 交叉编译)

## Git 分支模型

- **`main` 是稳定线**: 始终可发布, 作为版本发布基线。功能与修复合入 main 后即进入发布候选状态
- 功能 / 修复 / CI / 文档改动走独立分支, 分支名带前缀: `feat/xxx`、`fix/xxx`、`ci/xxx`、`docs/xxx`
- 单人开发时, 小改动 (typo、单文件 bug 修复、注释、脚本微调) 可直接 push main; 功能级改动建议走分支 + PR, 让 CI 全量门禁把关并留下可回溯的记录
- 合并方式 squash 或保留历史均可, 但 **main 必须保持线性**: 合入前 rebase 到最新 main, 不要引入合并分叉 (no-ff 合并)
- **main 禁止 force push**: 任何情况 (包括 rebase 后补推) 都不得改写 main 历史。main 上的提交只能追加
- tag (`v*`) 只在 main 上打, 且只指向正式发布提交

## 提交信息规范

提交信息使用 Conventional Commits, 格式:

```
type(scope): desc
```

- **type**: `feat` / `fix` / `refactor` / `docs` / `ci` / `chore` / `test`
- **scope**: 影响的模块, 可选但推荐。常用值: `core` (can-monitor-core)、`socketcan`、`usbvci`、`tauri`、`web`、`server` (can-monitor-server)、`types` (can-types)、`devices` (can-devices)、`ci` 等
- **desc**: 中文或英文均可, 但仓库内格式统一; 用一般现在时祈使句 (add / fix / extract), 动词开头
- 单行标题 ≤ 72 字符
- 不提交中间态: 一个提交一个逻辑单元, 每个提交都应是可编译、测试可通过的状态, 不留 "wip" 提交
- 示例:
  - `feat(core): fan-out broadcast layer with bounded consumer queues`
  - `fix(tauri): include icon.ico in bundle.icon for Windows bundler`
  - `docs: ssh-x11 remote usage guide + usbcan udev rule`
  - `chore(release): v0.1.1`

## 不提交清单

以下内容**不得**进入公开仓库 (已在 .gitignore 忽略):

- `.omo/` — Sisyphus 工作流内部状态 (plans / notepads / e2e / evidence)
- `CAN分析仪资料20250624_Linux/` — 供应商 SDK 源目录 (334MB, 含厂商安装包)
- `target/` — 本地构建产物

例外: `third_party/controlcan/` 内的厂商库 (`libcontrolcan.{a,so}` 与 `ControlCAN.dll`) **随源码提交**, 这是有意为之 — 发行包需要它们开箱即用 (来源与许可见 [docs/VENDOR.md](docs/VENDOR.md))。本地打包产物目录 (如 `releases/`) 不入库。

提交前 `git status` 复核, 只 stage 本逻辑单元涉及的文件, 不要夹带 Cargo.lock 或他人文件 (仓库有并行任务史)。

## 代码质量门禁

提交前本地必须全绿:

```bash
cargo test --workspace                     # 全部单元测试
cargo clippy --workspace --all-targets -- -D warnings   # 零告警
cargo fmt --check                          # 格式
bash scripts/check-core-purity.sh          # core 无 UI 依赖 + 依赖白名单
cargo doc --no-deps                        # rustdoc 无新警告
```

合并 (PR) 门禁:

- **CI 全绿才合并**: `ci.yml` 三平台 (Ubuntu / Windows / macOS) cargo check + Linux 全量门禁 (test / clippy / fmt / core 纯度 / vcan 测试)
- core crate 纯度检查是硬门: `can-monitor-core` 禁止 UI 依赖 (ratatui / crossterm), 依赖仅限白名单 (`can-types` / `canopen-stack` / `j1939-stack` / `crossbeam-channel`), 由 `scripts/check-core-purity.sh` 自动校验

新增 `can-monitor-core` 代码时遵守核心约束: 同步、无 UI 依赖、依赖白名单。can-usbvci 的 FFI 层是唯一的 unsafe 来源, 新 unsafe 需在注释里说明安全性。

## 测试要求

- 新逻辑必须带单元测试; 后端行为用 trait 抽象注入测试桩 (参考 `can-usbvci` 的 `VciOps` mock 与 `can-monitor-core` 的 `MockBackend`)
- 涉及 Web API 的改动跑 `cd web && npm run test:e2e` (需要先起后端)
- 改动帧 JSON 契约 (ts/id/ext/dir/data/protocol/summary) 属于破坏性变更, 需同步 README 与 docs/web-api.md, 并跑三形态对齐验证

## PR 流程

1. 在功能分支提交改动, 推送前跑完全部门禁
2. 开 PR 时说明: 改动内容、验证方式 (测试/实机/CI)、以及是否影响三形态 (TUI / Web / GUI) 的一致性
3. CI 全绿后由维护者 review 合入。合入用 squash merge, 提交信息保持 conventional 风格

## 发布

版本语义 (SemVer)、双版本号同步、发布步骤、hotfix 流程与触发机制见 [docs/release-process.md](docs/release-process.md)。发布由 tag push 自动触发, 开发者一般不需要手动碰 CI。
