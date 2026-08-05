# Contributing

欢迎参与 can-monitor 的开发。项目很小但门禁严格,改动前先读这几条。

## 环境

- Rust stable (建议 ≥ 1.88),workspace 根目录 `cargo build` 应零错误
- Linux 需要 CAN 工具链做 vcan 联调 (可选,`scripts/vcan-setup.sh`)
- GUI 形态需要 webkit2gtk / dbus 系统依赖 (见 README),本机装不上时用 CI 验证
- 仓库自带 devcontainer,包含全部工具链 (rustc / zig / can-utils / 交叉编译)

## 分支与提交

- 开发基于 `feat/*` 分支,合入前先 rebase 到最新 `main`
- 提交信息用 conventional commits: `feat:` / `fix:` / `refactor:` / `test:` / `docs:` / `ci:` / `chore:`
- 每次提交只包含逻辑相关的文件。仓库有并行任务史,提交前 `git status` 复核,不要夹带 Cargo.lock 或他人文件
- 不要提交 `.omo/`、`target/`、`CAN分析仪资料*` 等目录 (已在 .gitignore)

## 质量门禁 (合并前必须全绿)

```bash
cargo test --workspace                     # 全部单元测试
cargo clippy --workspace --all-targets -- -D warnings   # 零告警
cargo fmt --check                          # 格式
bash scripts/check-core-purity.sh          # core 无 UI 依赖 + 依赖白名单
cargo doc --no-deps                        # rustdoc 无新警告
```

新增 `can-monitor-core` 代码时遵守核心约束: 同步、无 UI 依赖、依赖仅限白名单 (`can-types` / `canopen-stack` / `j1939-stack` / `crossbeam-channel`)。can-usbvci 的 FFI 层是唯一的 unsafe 来源,新 unsafe 需在注释里说明安全性。

## 测试要求

- 新逻辑必须带单元测试;后端行为用 trait 抽象注入测试桩 (参考 `can-usbvci` 的 `VciOps` mock 与 `can-monitor-core` 的 `MockBackend`)
- 涉及 Web API 的改动跑 `cd web && npm run test:e2e` (需要先起后端)
- 改动帧 JSON 契约 (ts/id/ext/dir/data/protocol/summary) 属于破坏性变更,需同步 README 与 docs/web-api.md,并跑三形态对齐验证

## PR 流程

1. 在 `feat/*` 分支提交改动,推送前跑完全部门禁
2. 开 PR 时说明: 改动内容、验证方式 (测试/实机/CI)、以及是否影响三形态 (TUI / Web / GUI) 的一致性
3. CI 全绿后由维护者 review 合入。合入用 squash merge,提交信息保持 conventional 风格
