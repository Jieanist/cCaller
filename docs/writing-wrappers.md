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

## 宏驱动写法（路线图，M4 `gen`）

hitest 里的 `export_function.h` + 生成器允许「用宏定义函数 → 自动生成 TOML」，避免手抄
`libs.toml`。cCaller 计划提供等价能力：一个宏头 + `ccaller gen` 子命令，用宏同时声明函数名、
参数名与 slot_roles，生成器扫描源码派生出 `libs.toml`：

```c
// 设想中的用法（尚未落地）
CCALLER_FUNC(malloc, len, mem_idx:write) { /* ... */ }
```

此功能落在 M4 的 `gen` 子命令，落地后本文档会补完整示例与说明。
