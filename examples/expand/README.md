# expand 验收语料

这是 M4 `expand` 子命令的验收语料：三份用例配置，专门覆盖输入展开的边界情况。
每份配置的注释里写明了**期望展开出的子用例**（名称与顺序），供 `expand` 子命令
输出做逐条核验。

## 覆盖点

| 文件 | 覆盖 |
|---|---|
| `refs_list.toml` | `shared_inputs` + `refs`、列表、`$var` 在 cmd args/expect 解析、own-args 闭区间边界用 `$var` 引用单值 shared 参数 |
| `range_cartesian.toml` | 闭区间、单组内多参数**笛卡尔积**、参数名**排序确定性**（首参数变化最慢） |
| `multi_group.toml` | 多输入组**独立**展开（不跨组笛卡尔积）+ 声明顺序 |

## 子用例命名与顺序规则（Q-05 / FR-C-05）

- 命名：`{test}/{group}#{index}[k=v,…]`，`k` 按参数名排序。
- 无输入组的测试 → 1 个子用例，名为裸测试名。
- 单组内多参数 → 按参数名排序做笛卡尔积，**第一个排序参数变化最慢**。
- 多组 → 各组独立展开，按声明顺序输出。

## 说明

- 三份用例共用 `libs.toml`（只声明 `Call_add(a,b)`，无 param_page 槽位，避免
  def-use 干扰，专注展开语义）。
- `expect_eq = 0` 是语法占位（只为满足“测试 Cmd 必须带断言”），这些配置用于
  `check`/`expand` 核验展开形状，**不用于 `run`**（真正可跑的端到端用例在
  `../libc_wrapper/`）。

## 核验方式（当前 M3 二进制只到 `check`，`expand` 待 M4）

```sh
ccaller --test examples/expand/refs_list.toml       --lib examples/expand/libs.toml check
ccaller --test examples/expand/range_cartesian.toml --lib examples/expand/libs.toml check
ccaller --test examples/expand/multi_group.toml     --lib examples/expand/libs.toml check
```

`check` 输出里的 `N subcases` 应分别等于 5（refs_list：2+3）、6、3。
