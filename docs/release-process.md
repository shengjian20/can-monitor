# 发布流程与版本管理

can-monitor 的版本管理遵循 [语义化版本](https://semver.org/lang/zh-CN/) (SemVer), 发布由 git tag 自动触发, 产物为 GitHub Releases draft, 人工核对后正式发布。本文档面向仓库维护者, 描述版本号规则、发布步骤、紧急修复 (hotfix) 流程与各 CI 的触发机制。日常协作规范见 [CONTRIBUTING.md](../CONTRIBUTING.md)。

## 版本语义 (SemVer)

版本号格式 `MAJOR.MINOR.PATCH`:

- **PATCH** — bug 修复、新增平台构建 (如新增 arm64 发布), 严格向后兼容
- **MINOR** — 向后兼容的新功能 (新后端、新协议、新形态)
- **MAJOR** — 破坏性变更 (CLI 参数不兼容、帧 JSON 契约变更、core 依赖白名单变动等)

## 版本号同步位置

版本号有**两处**, 发布时必须同时修改且保持一致:

| 文件 | 字段 |
|------|------|
| `src-tauri/Cargo.toml` | `package.version` |
| `src-tauri/tauri.conf.json` | `version` (顶层) |

两处不一致会导致 Tauri 打包产物的版本与 crate 版本错位。当前版本 `0.1.0`, tag 为 `v0.1.0` (tag 带 `v` 前缀, 版本号本身不带)。

## 发布流程 (标准发布)

1. **更新 CHANGELOG.md**: 把 `[Unreleased]` 节内容归入新版本节 `[x.y.z]`, 补上日期与版本对比链接 (Keep a Changelog 风格)
2. **bump 版本号**: 同步修改 `src-tauri/Cargo.toml` 与 `src-tauri/tauri.conf.json` 两处 `version`
3. **提交**: 单独一个提交 `chore(release): v0.1.x`, 只含上述改动, 不夹带其他文件
4. **打 tag 并推送**: `git tag v0.1.x && git push origin v0.1.x`。tag 只在 main 上打, 只推 `v*` 格式
5. **自动构建**: `release.yml` 由 tag push 触发, 在 `ubuntu-latest` (x86_64) / `ubuntu-24.04-arm` (aarch64) / `windows-latest` 三个 runner 上经 tauri-action 构建 (Linux: AppImage / deb / rpm; Windows: MSI / NSIS)
6. **人工核对发布**: 构建产物以 **draft release** 上传, 维护者核对三平台产物齐全后手动 Publish 正式发布

## 紧急修复 (hotfix) 流程

1. 从 `main` 拉出修复分支: `git checkout -b fix/xxx main`
2. 修复 + 本地全部门禁 (test / clippy / fmt / core 纯度) + 相关测试
3. 合并回 `main` (squash 或线性合并均可, 保持 main 线性, 禁 force push)
4. bump PATCH 版本号 (双位置) → 更新 CHANGELOG → 提交 `chore(release): v0.1.x`
5. 打 tag `v0.1.x` + push → 自动发布 (同标准流程)

紧急且不需要出包时, 也可以不发新 tag, 只把修复合并回 main (CI 仍会验证), 下一版发布时统一带上。

## 触发机制一览

| Workflow | 触发条件 | 作用 |
|----------|----------|------|
| `ci.yml` | push / PR (任何分支) | 三平台 cargo check + Linux 全量门禁, 合并前的质量闸 |
| `release.yml` | 仅 tag push (`v*`), 或手动 workflow_dispatch | 构建发行包, 产物为 draft release |
| `docker.yml` | tag push (`v*`) + main push | 构建 Docker 镜像到 ghcr.io (`<tag>` 镜像 + `latest`) |

- **发布只由 tag 触发**: 普通 push 永远不会触发 release.yml, 避免误发
- `workflow_dispatch` 只是手动试跑通道 (此时 `github.ref_name` 是分支名), 正式发布必须走 tag push
- macOS 不在发布矩阵: Tauri v2 的 macOS 产物需要 Apple 签名/公证 secrets, 项目未配置 (release.yml 内有扩展说明)
