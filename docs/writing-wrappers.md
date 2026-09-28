# 编写 wrapper（.so / .dll）

wrapper 是被测 SDK 的动态库，向框架暴露统一调用签名。核心契约定义在
[crates/ffi/include/ccaller.h](../crates/ffi/include/ccaller.h)，这是唯一权威来源。

## ABI 握手

每个 wrapper 必须导出：

```c
int64_t CCaller_abi_version(void) { return CCALLER_ABI_VERSION; }  // 返回 1
```

框架加载时调用它核对版本，不一致就拒绝加载并打印双方版本号。

## 统一调用签名

每个被测函数导出为 `Call_<name>`：

```c
int64_t Call_<name>(uint64_t *param_page, const int64_t *params, int64_t param_len);
```

- `param_page`：框架持有的 512 个 `uint64_t` 槽位页，每个 worker 线程独立一页。
  一次调用写入的值，对**同一线程**的后续命令可见（跨线程绝不共享）。
- `params`：本次调用的只读参数，按 `paras` 声明的顺序排列。字符串参数是指针，
  生命周期仅覆盖本次调用，需要留存就自己拷贝。
- `param_len`：`params` 的元素个数，应等于声明的参数个数。

## 返回值分区

| 值 | 归属 | 含义 |
|---|---|---|
| `0` | 框架 | 成功 |
| `[-127, -1]` | wrapper | 自定义失败；框架判失败并原样透传该数值 |
| `[-255, -128]` | 框架 | 保留区间；wrapper 禁止返回（当前只有 `CCALLER_ERR_SKIP = -255`） |
| `> 0` | 保留 | 未定义；按失败透传 |

- 跳过当前命令：返回 `CCALLER_ERR_SKIP`（-255），框架继续下一条命令、计入 skipped、不影响退出码。
- 自定义失败：返回 `[-127, -1]` 区间内的数值，框架计入 failure。

## param_page 与 slot_roles

`param_page` 的下标访问必须在 `[0, 512)` 内做边界检查。参数里哪些是「页下标」、是读还是写，
由 `libs.toml` 的 `slot_roles` 显式声明，框架据此在加载期做「读前必写」的静态检查：

```toml
funcs = [
  { name = "Call_malloc", paras = ["len", "mem_idx"], slot_roles = { mem_idx = "write" } },
  { name = "Call_read32", paras = ["addr_idx"],      slot_roles = { addr_idx = "read"  } },
]
```

`slot_roles` 取值：`read` / `write` / `read_write`。

## 宏驱动写法（`ccaller gen`）

手写 `Call_*` 签名容易和 `libs.toml` 脱节。改用 `ccaller_gen.h` 的宏声明函数，让
`ccaller gen` 扫描源码自动生成库描述——wrapper 源码是唯一事实来源：

```c
// wrapper.c
#include "ccaller_gen.h"

int64_t CCaller_abi_version(void) { return CCALLER_ABI_VERSION; }

CCALLER_FUNC(malloc, len, mem_idx:write)
{
    /* 函数体，直接使用 param_page / params / param_len */
    return CCALLER_OK;
}

CCALLER_FUNC(add, a, b)
{
    return params[0] + params[1];
}
```

宏参数语法：`name` 是函数名（导出 `Call_<name>`）；`ident` 是值参数；`ident:read` /
`ident:write` / `ident:read_write` 是 param_page 槽位下标参数。可变尾参只是扫描器元数据，
不进入展开，不影响编译。

生成：

```sh
ccaller gen wrapper.c               # 生成 ./libs.toml
ccaller gen wrapper.c -o libs.toml  # 指定输出路径
```

生成结果：

```toml
version = 1
[[libs]]
path = "wrapper.so"     # 源文件 stem + 平台扩展；Windows 为 .dll
funcs = [
  { name = "Call_malloc", paras = ["len", "mem_idx"], slot_roles = { mem_idx = "write" } },
  { name = "Call_add",    paras = ["a", "b"] },
]
```

扫描器按 C 翻译阶段顺序处理：先做反斜杠-换行拼接，再把注释/字符串内容按字节等长掩码
（偏移不变 → 行列精确），跳过 `#define` 行与异平台条件编译块（`#else` 正确保留、嵌套 `#if`
用栈维护）。生成的 `libs.toml` 可直接配合用例配置跑 `ccaller check`。

端到端可运行样例见 [examples/gen/](../examples/gen/README.md)。
