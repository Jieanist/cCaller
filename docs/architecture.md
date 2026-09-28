# 架构

依赖方向单向：`cli → core → ffi`。

| crate | 职责 |
|---|---|
| `crates/core` | 领域层：解析、校验、输入展开、静态 def-use 分析、执行调度、报告 |
| `crates/cli` | 薄壳：参数解析、日志引导、退出码 |
| `crates/ffi` | 唯一的 unsafe：dlopen/dlsym、ABI 握手、param_page |

## 关键设计

- **slot_roles 显式**：不按参数名猜测 param_page 下标，读写角色由 `libs.toml` 声明，
  加载期做「读前必写」静态检查。
- **param_page 每线程一页**：512 槽位固定，无共享可变状态；并发安全。
- **四层 env 顺序固定**：process → global → case → thread（退出逆序），单一 `ENTRY_ORDER` 常量，
  分析器与执行器共用，避免漂移。
- **返回值分区**：0=成功 / [-127,-1]=wrapper 自定义失败 / -255=skip / [-255,-128]=框架保留，
  与断言解耦。
- **断言注册表**：每个 `expect_*` 是一个独立文件 + 注册表登记一行；取反折叠双向闭合
  （ge↔lt、gt↔le）。
- **报告契约**：`check`/`expand` 的 `--format json` 是机器可读契约（`schema=1`），核心可测、
  CLI 只做转发。
- **死亡测试隔离**：子进程重执行（而非 fork-in-threads），避免多线程进程 fork 的死锁风险。
