# 功能特性

## 四层 env

env 作用域由外到内、进入顺序 process → global → case → thread，退出逆序：

| 层 | 配置 | 粒度 |
|---|---|---|
| process | `[process_env]` | 一次运行 |
| global | `[env]`（顶层） | 一次运行 |
| case | `[env]`（`[[envs]]` 绑定用例） | 每个测试 |
| thread | `[thread_env]` | 每个 worker 线程 |

`init` 失败会停掉后续作用域的 init（已进入的作用域 exit 仍会跑）；`exit` 失败计入失败。

## 断言

六种（互斥）：`expect_eq` / `expect_ne` / `expect_ge` / `expect_gt` / `expect_le` / `expect_lt`。

- 取反折叠：`!expect_ge → expect_lt`、`!expect_gt → expect_le`（双向闭合）。
- 返回值先按分区分类（成功 / 跳过 / 保留区间违约），再断言。

## 并发

- `thread_num`（测试级）：worker 线程数；`thread_num>1` 时子用例名带 `@k`。
- `[[concurrences]]`（顶层）：把多个测试编组并行。
- `-m/--max-thread`（CLI）：全局并发上限。
- `serial = true`：测试串行（只串行测试内 subcase，不影响 concurrences 组）。
- param_page 每 worker 一页，无共享可变状态。

## 死亡测试

`should_panic = true`：框架把该（子）用例放进子进程重执行（避免 fork-in-threads）。
崩溃（Unix 信号 / Windows NTSTATUS）判通过；正常返回或超时判失败；无隔离启动器则 skipped。
CLI 隐藏 `--isolate-test` / `--isolate-subcase` 成对控制隔离方式。可与普通子用例在同一测试内
混合（执行器按子用例分区）。

## perf 计时

命令 `perf = true` → 报告里记录 `duration_ns`（只统计调用本身）。`--format json` 可见。

## debug 过滤

- 配置 `debug_test = ["t1", "t2"]` 只跑这些测试。
- CLI `-d/--debug <name>` 可重复。
- 优先级 isolate > debug_test > CLI -d；未命中 → 退出码 2。

## 报告

`run --format text|json|junit`：

- `text`：控制台摘要（Total / Success / Failure / skipped + 失败清单）。
- `json`：`schema=1`，含 summary / cases / perf。
- `junit`：XML，含 `system-out`。
