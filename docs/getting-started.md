# 五分钟上手

从零跑通一个最小例子：写 wrapper → 写配置 → `check` → `run`。

## 1. 构建

需要 Rust 1.98+（MSRV）：

```sh
cargo build --release   # 产物 target/release/ccaller
```

## 2. 写 wrapper

wrapper 是被测 SDK 的动态库，把 SDK 接口包装成统一调用签名。最小例子：

```c
// wrapper.c
#include "ccaller.h"

int64_t CCaller_abi_version(void) { return CCALLER_ABI_VERSION; }

int64_t Call_add(uint64_t *param_page, const int64_t *params, int64_t param_len) {
    (void)param_page; (void)param_len;
    return params[0] + params[1];
}
```

编译：

```sh
gcc --shared -fPIC -I crates/ffi/include wrapper.c -o libwrapper.so
```

详见 [编写 wrapper](writing-wrappers.md)。

## 3. 写配置

库描述（声明动态库与导出函数）：

```toml
# libs.toml
version = 1
[[libs]]
path = "libwrapper.so"
funcs = [ { name = "Call_add", paras = ["a", "b"] } ]
```

用例配置（声明测试与命令序列）：

```toml
# cases.toml
version = 1
[[tests]]
name = "t_add"
cmds = [ { opfunc = "Call_add", expect_eq = 7, args = ["a=3", "b=4"] } ]
```

详见 [测试用例与配置参考](test-cases.md)。

## 4. 校验 + 执行

```sh
ccaller --test cases.toml --lib libs.toml check   # 加载期校验 + 静态 def-use 分析，0 findings
ccaller --test cases.toml --lib libs.toml run     # 执行 + 控制台摘要
```

- `check` 不执行任何命令，只做校验与展开统计。
- `run` 是默认子命令，不写也等价。
- 退出码：`0` 全部通过（允许 skipped）、`1` 有用例失败、`2` 配置/用法错误、`3` 框架内部错误。

更完整的端到端例子见 [examples/libc_wrapper/](../examples/libc_wrapper/README.md)。
