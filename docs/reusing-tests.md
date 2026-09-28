# 复用与参数化

一条命令要跑多组数据时，不必复制命令——用 `shared_inputs` 定义值集，用输入组 `refs` 引用，
框架按笛卡尔积展开成子用例。

## shared_inputs

```toml
[shared_inputs.common]
val = ["888", "999"]
```

名字 `common` 可被任何测试的输入组 `refs` 引用；`$var` 在命令参数里替换。

## 输入组 inputs

```toml
[[tests]]
name = "test_rw_u32"
cmds = [
  { opfunc = "Call_malloc", expect_eq = 0, args = ["len=100", "mem_idx=1"] },
  { opfunc = "Call_read32", expect_eq = "$val", args = ["addr_idx=1"] },
]
[[tests.inputs]]
name = "ipt1"
refs = ["common"]   # 复用 shared_inputs.common → val ∈ {888,999} → 2 个子用例
```

## 值的形式

输入组 `args` 里每个参数可取：

- 单值：`x = 1`
- 列表：`x = [1, 2, 3]`
- 闭区间：`x = { start = 1, end = 3, step = 1 }`（start/end 支持 `$var`；step 必填且 >0）
- `refs`：整组引用某个 `shared_inputs`

## 展开规则

- 同一输入组内多参数取笛卡尔积（参数名排序，第一个参数变化最慢）。
- 多个输入组独立展开（各自产生子用例），保持声明顺序。
- 子用例命名：`test名/组名#k[param=val]`；`thread_num>1` 时副本带 `@k`。

`expand` 子命令会打印完整展开清单，方便核对：

```
ccaller --test cases.toml --lib libs.toml expand --format json
```

## 组级覆盖

输入组可覆盖测试级的执行属性：

```toml
[[tests.inputs]]
name = "happy"
args = { v = [1, 2] }
break_if_fail = false     # 该组失败不中断

[[tests.inputs]]
name = "death"
args = { v = [0] }
should_panic = true       # 该组走死亡测试（子进程隔离）
```

同一测试内可混合普通组与死亡组；未指定的组继承测试级 `should_panic` / `break_if_fail`。
