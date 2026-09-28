# cCaller

通用 C 接口测试执行框架：用 TOML 描述被测动态库（wrapper）与测试用例，框架负责加载期校验、
输入展开、静态槽位 def-use 分析与并发执行调度。

- **当前状态**：M4 —— 并发执行、死亡测试隔离、perf、debug、JSON/JUnit 报告，以及
  `expand`/`init`/`fmt`/`gen` 开发者工具、六种断言、InputGroup 组级覆盖。
- **路线图**：`migrate`（hitest→cCaller 迁移）。

## 构建

需要 Rust 1.98+（MSRV）：

```
cargo build --release
```

产物为 `target/release/ccaller`（Windows 下 `ccaller.exe`）。

## 五分钟上手

**1. 写一个 wrapper**（被测 SDK 的动态库，遵守 [wrapper ABI](docs/writing-wrappers.md)）：

```c
// wrapper.c
#include "ccaller.h"

int64_t CCaller_abi_version(void) { return CCALLER_ABI_VERSION; }

int64_t Call_add(uint64_t *param_page, const int64_t *params, int64_t param_len) {
    (void)param_page; (void)param_len;
    return params[0] + params[1];
}
```

```
gcc --shared -fPIC -I crates/ffi/include wrapper.c -o libwrapper.so
```

**2. 写两份 TOML**（[配置参考](docs/test-cases.md)）：

```toml
# libs.toml —— 库描述
version = 1
[[libs]]
path = "libwrapper.so"
funcs = [ { name = "Call_add", paras = ["a", "b"] } ]
```

```toml
# cases.toml —— 用例配置
version = 1
[[tests]]
name = "t_add"
cmds = [ { opfunc = "Call_add", expect_eq = 7, args = ["a=3", "b=4"] } ]
```

**3. 校验 + 执行**：

```
ccaller --test cases.toml --lib libs.toml check   # 校验 + 静态分析，不执行
ccaller --test cases.toml --lib libs.toml run     # 并发执行 + 报告
```

## 功能

- **并发执行**：`thread_num` / `concurrences` / `-m`，每 worker 独立 param_page。
- **死亡测试**：`should_panic` 子进程隔离，崩溃即通过。
- **四层 env**：process / global / case / thread。
- **参数化**：`shared_inputs` + `refs` + 列表/区间 + `$var`。
- **断言**：`expect_eq` / `expect_ne` / `expect_ge` / `expect_gt` / `expect_le` / `expect_lt`。
- **报告**：`run --format text|json|junit`。
- **开发者工具**：`expand`（展开清单）/ `init`（脚手架）/ `fmt`（规范化）/ `gen`（宏 → 生成 TOML）。

完整文档见 [docs/](docs/README.md)，端到端示例见 [examples/libc_wrapper/](examples/libc_wrapper/)。

## 仓库结构

| 目录 | 说明 |
|---|---|
| `crates/core` | 领域层：解析、校验、展开、def-use、执行、报告 |
| `crates/cli` | `ccaller` 命令行 |
| `crates/ffi` | 动态库加载、ABI 契约、param_page |
| `examples/` | 端到端示例与验收语料 |
| `docs/` | 文档 |

## License

BSD-2-Clause
