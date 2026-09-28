# cCaller

通用 C 接口测试执行框架：用 TOML 描述被测动态库与测试用例，框架负责加载期校验、
输入展开、静态槽位 def-use 分析与执行调度。

> **当前状态**：里程碑 M3 —— 并发执行、死亡测试隔离、perf 计时、debug 过滤
> 与 JSON/JUnit 报告已落地（`ccaller run` 默认并发）。
> `expand`/`gen`/`init`/`fmt`/`migrate` 等其余子命令随后续里程碑落地。

## 构建

需要 Rust 1.98+（MSRV）：

```
cargo build --release
```

产物为 `target/release/ccaller`（Windows 下为 `ccaller.exe`）。

## 五分钟上手

写两份 TOML：**库描述**（声明被测动态库与导出函数）与**用例配置**（声明测试与命令序列）。

```toml
# libs.toml —— 库描述
version = 1

[[libs]]
path = "wrapper.dll"   # .so（Linux）/ .dll（Windows），相对本文件
funcs = [
  { name = "Call_malloc", paras = ["len", "mem_idx"], slot_roles = { mem_idx = "write" } },
  { name = "Call_read32", paras = ["addr_idx"],      slot_roles = { addr_idx = "read" } },
]
```

```toml
# cases.toml —— 用例配置
version = 1

[shared_inputs.common]
val = ["888", "999"]

[[tests]]
name = "test_rw_u32"
cmds = [
  { opfunc = "Call_malloc", expect_eq = 0,      args = ["len=100", "mem_idx=1"] },
  { opfunc = "Call_read32", expect_eq = "$val", args = ["addr_idx=1"] },
]
[[tests.inputs]]
name = "ipt1"
refs = ["common"]   # 复用 shared_inputs.common → 2 个子用例：val ∈ {888, 999}
```

检查配置（不执行任何命令）：

```
ccaller --test cases.toml --lib libs.toml check
```

输出：

```
ok: 1 tests, 2 subcases, 4 commands
```

机器可读输出：

```
ccaller --test cases.toml --lib libs.toml check --format json
```

## CLI

全局选项（与 `--help` 输出一致）：

| 选项 | 说明 |
|---|---|
| `-t, --test <FILE>` | 用例配置文件；兼容别名 `--test_case` |
| `-i, --lib <FILE>` | 库描述文件；兼容别名 `--input` |
| `-l, --log <LEVEL>` | 日志级别：1-4（error/warn/info/debug）或级别名；`RUST_LOG` 优先 |
| `-d, --debug <NAME>` | 仅执行名称匹配的测试；可重复（优先级低于配置 `debug_test` 与隔离） |
| `-m, --max-thread <N>` | 并发 worker 上限（≥1）；受用例 `thread_num`/`concurrences` 约束 |
| `-h, --help` | 打印帮助 |
| `-V, --version` | 打印版本 |

子命令：

| 子命令 | 说明 |
|---|---|
| `run`（默认） | 并发执行子用例（`thread_num`/`concurrences`/`-m` 控并发），四层 env，死亡测试子进程隔离，控制台摘要；`--format text\|json\|junit`；`--allow-empty` 允许 0 用例退出 0 |
| `check` | 加载期校验 + 静态槽位 def-use 分析 + 展开数量统计，不执行；`--format text\|json`（默认 `text`） |

执行（不写子命令即默认 `run`）：

```
ccaller --test cases.toml --lib libs.toml
```

`expand`、`gen`、`init`、`fmt`、`migrate` 等其余子命令随后续里程碑落地。

## 退出码

| 码 | 含义 |
|---|---|
| 0 | 全部通过（允许存在 skipped） |
| 1 | 存在用例失败（断言失败、env 失败）；0 用例执行且未加 `--allow-empty` |
| 2 | 配置/环境错误：加载期校验失败、库加载失败、用法错误、文件不可读、debug 过滤未命中 |
| 3 | 框架内部错误（bug） |

## 配置格式

**库描述**声明动态库路径与每个导出函数的参数名列表；参数若表示 `param_page`
的下标，用 `slot_roles` 标注它是被写还是被读（框架据此在加载期做
"读之前必须有人写过"的静态检查）。

**用例配置**声明测试、命令序列与输入组；输入组支持单值、列表与闭区间，
同一组内多值参数按笛卡尔积展开，子用例名形如 `test_rw_u32/ipt1#0[val=888]`。

上面的示例覆盖了最常用的字段。更完整的字段以 `ccaller check` 的输出为准：
每条诊断都会给出文件名、行号、列号与可读的说明。

## 仓库结构

| 目录 | 说明 |
|---|---|
| `crates/core` | 领域层：配置解析、校验、输入展开、def-use 分析 |
| `crates/cli` | `ccaller` 命令行 |
| `crates/ffi` | 动态库加载、ABI 契约、param_page（进行中） |
| `docs/` | 补充文档 |
