# 更新日志

本文件记录 cCaller 的显著变更，格式参照
[Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)；里程碑划分与
见下文。

## [Unreleased]

### M2：串行执行（`run` 子命令）

#### 新增

- `ccaller run` 子命令（默认子命令，FR-X-01）：加载库描述、ABI 握手、
  串行执行每个子用例，退出码契约 0/1/2/3（FR-X-02、需求 7.5）。
- 执行上下文 `Runner`（AR-03）：显式持有已加载库、param_page、计划与
  Reporter，无全局可变状态。
- 运行计划 `Plan`：库路径相对描述文件解析、env 四层（global/process/
  case/thread，Q-13 顺序）解析为可执行命令。
- `Reporter` 扩展点 + 控制台实现：Total/Success/Failure/skipped 摘要 +
  失败用例清单，结果走 stdout、日志走 stderr、无 ANSI 颜色（FR-R-01/02）。
- 断言语义：返回值分类（成功/跳过/保留区间违约）与断言求值；SKIP 为
  Cmd 级且不影响退出码（Q-01、FR-T-07）。
- env 失败可见性：init/exit 失败计入失败并影响退出码（FR-E-03、NFR-02）。
- 空运行规则：0 用例非零退出，`--allow-empty` 显式放行（Q-08）。

### 断言扩展点（FR-V-02、F-V-03）

#### 新增

- `Assertion` trait 与断言注册表：`expect_*` 字段按名路由到注册表，
  新增断言 = 新增文件 + 注册表登记一行（FR-V-02、G-02 / A-2）。
- `expect_ge` 比较断言（F-V-03）。

### M1：配置模型与 check

#### 新增

- 库描述模型（7.2）：严格解析（未知字段报错，FR-C-09）、
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
