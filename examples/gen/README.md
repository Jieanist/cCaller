# gen 宏驱动样例

这个目录演示 `ccaller gen` 的宏驱动工作流：wrapper 用 `CCALLER_FUNC` 宏声明
函数，`ccaller gen` 扫描源码生成 `libs.toml`——**wrapper 源码是唯一事实来源**，
不再手写 `Call_*` 签名 + 手写 `libs.toml` 两份会脱节的配置。

## 工作流

```sh
cd examples/gen
ccaller gen wrapper.c          # -> libs.toml（覆盖写，见 .gitignore）
sh build.sh                    # -> wrapper.so
ccaller --test cases.toml --lib libs.toml check
ccaller --test cases.toml --lib libs.toml run
```

## wrapper.c 覆盖的宏语法

| 函数 | 宏声明 | 说明 |
|---|---|---|
| `Call_add` | `CCALLER_FUNC(add, a, b)` | 两个值参数，返回 `a + b` |
| `Call_ret` | `CCALLER_FUNC(ret, v)` | 单值参数，原样回传（断言测试用） |
| `Call_store` | `CCALLER_FUNC(store, idx:write, v)` | `idx` 是**写**槽位下标 |
| `Call_load` | `CCALLER_FUNC(load, idx:read)` | `idx` 是**读**槽位下标 |
| `Call_bump` | `CCALLER_FUNC(bump, idx:read_write)` | `idx` 是**读-写**槽位下标，返回旧值并自增 |
| `Call_skip` | `CCALLER_FUNC(skip)` | 无参，返回 `CCALLER_ERR_SKIP` |
| `Call_boom` | `CCALLER_FUNC(boom, flag)` | `flag != 0` 时空指针崩溃（死亡测试演示） |

`ccaller gen` 据此生成 `libs.toml`，把 `:write` / `:read` / `:read_write` 后缀
映射成 `slot_roles`；纯标识符（无后缀）是值参数。

## 端到端预期

- `check`：`ok: 1 tests, 1 subcases, 6 commands`（0 findings）。
- `run`：退出码 0，`Total 1 / Success 1 / skipped 1`（`Call_skip` 记 skipped）。
