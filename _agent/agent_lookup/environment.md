# environment — 环境 / 工具链同一性轴（rustc / cargo / gcc / python；本项目的「硬件」等价物）

（验证于 2026-09-28）

> 本项目是纯软件（Rust 测试框架），无物理板卡。此文件承接 hardware.md 的角色，记录「同一事实在什么环境下成立」。

## 工具链
- rustc：项目 MSRV 1.98（workspace rust-version）；本机 `~/.cargo/bin` 为 1.85.0（不在默认 PATH）——本地构建需先装 1.98。
- 测试自包含：ffi/core 集成测试用 Rust cdylib fixture（tests/fixtures/*）即时编译，无需外部 gcc/msvc。

## 依赖与环境
- Cargo.lock 已纳入 git（pin 依赖 + rustc MSRV）。
- workspace 依赖：thiserror / serde / toml(0.9) / serde_json / clap(4) / log / env_logger / libloading。

## CI / 质量门
- .github/workflows/ci.yml：rustfmt / clippy(-D warnings) / test / build(release) / MSRV(1.98) / cargo-deny / perf baseline。

## 沙箱 / 权限
- 本仓库位于 `~/nfs/cCaller` = `/mnt/nvme/nfs/hja/cCaller`，/mnt/nvme 为只读挂载（ro）——写入记忆/代码需在可写环境或先 remount rw。

## 外部文档指针
- README.md（五分钟上手 / CLI / 退出码 / 配置格式）；CHANGELOG.md（里程碑）；docs/README.md（后续补字段参考/wrapper 指南/迁移说明）。
