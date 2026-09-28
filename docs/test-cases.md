# 测试用例与配置参考

配置是两份 TOML：**库描述**（libs.toml）与**用例配置**（cases.toml）。`version = 1` 必填；
未知字段一律报错（带文件 / 行 / 列）。

## 库描述 libs.toml

```toml
version = 1
[[libs]]
path = "libwrapper.so"     # .so / .dll / .dylib，相对本文件解析
funcs = [
  { name = "Call_malloc", paras = ["len", "mem_idx"], slot_roles = { mem_idx = "write" } },
  { name = "Call_read32", paras = ["addr_idx"],      slot_roles = { addr_idx = "read"  } },
]
```

- `funcs[].name`：导出符号名 `Call_<name>`。
- `funcs[].paras`：参数名列表；命令里 `args` 的 `name=value` 必须与之名称、个数、顺序一致。
- `funcs[].slot_roles`：把某些参数标为 param_page 下标（`read` / `write` / `read_write`），
  供静态 def-use 分析做「读前必写」。

## 用例配置 cases.toml

顶层字段：

| 字段 | 说明 |
|---|---|
| `version` | 必填，`1` |
| `[env]` | 用例级 env（可选） |
| `[thread_env]` | 线程级 env（可选） |
| `[process_env]` | 进程级 env（可选） |
| `[[envs]]` | 命名 env（可选，可绑定用例） |
| `[shared_inputs]` | 可复用输入集 |
| `[[tests]]` | 测试列表 |
| `[[concurrences]]` | 并发组 |
| `debug_test` | 调试过滤（只跑这些测试） |
| `default_serial` | 默认是否串行 |

### 测试 `[[tests]]`

```toml
[[tests]]
name = "test_rw_u32"      # 全局唯一
thread_num = 4            # worker 线程数（默认 1）
should_panic = false      # 死亡测试（默认 false）
break_if_fail = true      # 命令失败即停（默认 true）
serial = false            # 强制本测试串行（默认继承 default_serial）
cmds = [
  { opfunc = "Call_malloc", expect_eq = 0, args = ["len=100", "mem_idx=1"] },
]
[[tests.inputs]]          # 输入组（参数化）
name = "ipt1"
refs = ["common"]
```

### 命令 `cmds[]`

```toml
{ opfunc = "Call_add", expect_eq = 7, args = ["a=3", "b=4"], perf = true }
```

- `opfunc`：调用的导出函数名。
- `args`：`name=value` 列表；`value` 可用 `$var` 引用共享输入。
- 断言（互斥，六选一）：`expect_eq` / `expect_ne` / `expect_ge` / `expect_gt` / `expect_le` / `expect_lt`。
  取反折叠：`!expect_ge → expect_lt`、`!expect_gt → expect_le`。
- `perf = true`：记录本次调用耗时（报告里 `duration_ns`）。
- `timeout`：本次调用的超时秒数，默认 `60`；设为 `0` 表示不限时。某条命令超过预算仍不返回时，
  cCaller 的 watchdog 会认定是 wrapper/驱动卡死（FFI 调用未返回），向 stderr 打印诊断并以退出码
  `1`（用例失败）退出进程，避免 CI 机器被无限占用。适用于 `cmds[]`、`env`/`thread_env`/`process_env`
  的 `init`/`exit` 里所有命令。

### 输入组 `inputs[]`

```toml
[[tests.inputs]]
name = "ipt1"          # 输入组名
refs = ["common"]      # 复用某个 shared_inputs
# 或直接声明参数：
#   args = { val = [1, 2, 3] }
should_panic = true    # 可选：覆盖测试级默认值
break_if_fail = false  # 可选：覆盖测试级默认值
```

`should_panic` / `break_if_fail` 未指定时继承测试级；展开后不同组可混合死亡 / 普通执行。
展开规则见 [复用与参数化](reusing-tests.md)。

### env

`[env]` / `[thread_env]` / `[process_env]` 结构相同：`init` / `exit` 命令列表。
`[[envs]]` 额外有 `name` 与 `tests`（绑定到哪些测试）。执行顺序与粒度见
[功能特性](features.md)。

### 并发组 `[[concurrences]]`

```toml
[[concurrences]]
name = "parallel_mem"
tests = ["t_a", "t_b"]   # 这两个测试编组并行
```
