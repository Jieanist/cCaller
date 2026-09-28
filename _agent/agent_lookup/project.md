# project — 慢变事实（真·不变才写；可从代码/现场推导的不写）

（验证于 2026-09-28）

## 目标（3 行）
- 通用 C 接口测试执行框架：TOML 描述被测动态库与用例，框架负责加载期校验、输入展开、静态槽位 def-use 分析、执行调度，与业务逻辑解耦。
- 是 hitest 的重新设计版：把 hitest「能跑」重构为可静态校验的工程化框架（分层 workspace + 严格解析 + 诊断带行列号）。
- 关键指标：退出码可被 CI 判定（0=全过 1=有失败 2=配置/环境错 3=框架 bug）；check 的 7.6 JSON 契约可机器消费。

## 架构
- workspace 分层，依赖方向 cli → core → ffi（AR-01）：`crates/core`（领域模型/校验/展开/def-use/调度/断言/报告）、`crates/ffi`（唯一 unsafe：dlopen/ABI 握手/param_page/调用桥）、`crates/cli`（薄壳）。
- 关键拓扑：CaseConfig（tests/envs/concurrences/shared_inputs/thread_env/process_env）→ 校验 → 展开(SubCase) → def-use → Plan → Runner(execute) → ffi::call::invoke → wrapper `Call_<name>`。
- `param_page` = ffi::page::ParamPage（512×u64），每 worker 线程一页（Q-06），静态分析与运行时共享 PARAM_PAGE_SLOTS 常量。

## 接口契约（≤15 行）
- FFI：`unsafe extern "C" fn(param_page: *mut u64, params: *const i64, param_len: i64) -> i64`（loader::CallFn）。
- 握手：wrapper 必须导出 `CCaller_abi_version() -> 1`（FR-A-04），不符拒载并报两个版本号。
- 返回值分区：0=OK；-255=SKIP（Cmd 级，不影响退出码）；[-127,-1]=wrapper 自定义失败；[-255,-128]=框架保留（wrapper 禁返）；>0=保留。
- param_page 共 512 个 u64（CCALLER_PARAM_PAGE_SLOTS），下标 [0,512)；slot 语义来自库描述 `slot_roles`（read/write/read_write），绝不猜参数名。
- 字符串参数指针仅在单次调用内有效（Q-07），wrapper 需自持拷贝。

## 关键约束（逐条带「违反会怎样」）
- 配置严格解析 `deny_unknown_fields`——未知/缺字段直接 load-time 报错（带文件:行:列）。
- def-use「读前必写」+ 下标范围——读未写 slot 报 SLOT_READ_BEFORE_WRITE，越界报 SLOT_OUT_OF_RANGE。
- 四层 env 顺序 process→global→case→thread（ENTRY_ORDER，Q-13），analyzer 与 runtime 共享同一常量防漂移。
- 断言经注册表路由（expect_eq/ne/ge）——`expect_ge` 否定折叠到 `expect_lt` 但未注册，写 `expect_ge="!7"` 是 load-time error（有测试锁定）。
- MSRV rustc 1.98（workspace rust-version）；本机 `~/.cargo/bin` 为 1.85，需装 1.98 工具链才能本地构建。

## 未实现（M3+，见 status.md；hitest 有、cCaller 尚未落地）
- 并发执行（thread_num/concurrences/-m max-thread）——当前串行，Plan 不携带 thread_num/concurrences。
- 死亡测试（should_panic）——当前直接 skip，无 fork/子进程隔离。
- perf 计时（CmdDef.perf 已解析但 ResolvedCmd 不携带、执行器不计时）。
- debug 过滤（debug_test 字段存在但 CLI 无 -d、执行器忽略）。
- run 的 JSON/JUnit 报告（仅 ConsoleReporter）；wrapper 便捷宏/配置生成器（gen crate 未建）。

## 工具链原则（只写非显然的坑，不复制手册命令）
- cargo/rustc 在 `~/.cargo/bin`（不在默认 PATH）；本机 1.85 vs 项目 MSRV 1.98。
- 测试自包含：ffi/core 集成测试用 Rust cdylib fixture（tests/fixtures/*）即时编译，无需外部 gcc/wrapper。

## 模块地图（域 → 路径 → 一行坑）
- config → crates/core/src/config/{source,diag,lib_desc,cases,validate,expand,defuse,check,value,layers}.rs → 严格解析 + span 定位 + 一次性报错
- run → crates/core/src/run.rs → 串行执行器；env 生命周期 Q-13；should_panic 目前 skip
- plan → crates/core/src/plan.rs → 展开成可执行计划；不携带 thread_num/concurrences
- report → crates/core/src/report.rs → Reporter trait；仅 ConsoleReporter 实现
- assertion → crates/core/src/assertion/ → 注册表扩展点（eq/ne/ge）
- ffi → crates/ffi/src/{loader,abi,call,page}.rs → 唯一 unsafe；握手 + 返回值分区 + 调用桥
- cli → crates/cli/src/{main,logging,exit_code}.rs → clap v4 子命令 check/run；退出码 0/1/2/3
