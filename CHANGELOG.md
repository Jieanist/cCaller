# 更新日志

本文件记录 cCaller 的显著变更，格式参照
[Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)；里程碑划分与
`docs/v1.0/需求说明书.md` §9 对齐。

## [Unreleased]

### M1：配置模型与 check

#### 新增

- 库描述模型（需求说明书 7.2）：严格解析（未知字段报错，FR-C-09）、
  `slot_roles` 显式槽位角色（禁止按参数名猜测）。
- 用例配置模型（7.3）：全局/用例级/线程/进程 env、`shared_inputs`、输入组。
- 加载期校验（FR-C-08）：交叉引用与语义规则一次报完，错误含文件名 + 行列号。
- 输入展开（FR-C-03~06）：单值/列表/闭区间、`refs` 复用、`$var` 解析、
  参数名排序的确定性展开与 Q-05 子用例命名。
- 静态槽位 def-use 分析（FR-C-10）：读前必写、下标范围 `[0, 512)` 校验。
- `ccaller check` 子命令（FR-X-03）：`--format text|json`（默认 `text`），
  JSON 为 7.6 机器可读契约；报告走 stdout，日志走 stderr。
- 全局选项 `-t/--test`、`-i/--lib`（保留兼容别名 `--test_case`/`--input`）。

### M0：骨架

#### 新增

- Cargo workspace（`core`/`ffi`/`cli`/`gen`）与工程基线
  （rustfmt/clippy 纪律、MSRV 1.98、行尾策略）。
- CLI 骨架：`--version`、`--help`、`-l/--log` 与 stderr 日志引导。
- 退出码契约（FR-X-02）。
