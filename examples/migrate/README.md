# hitest → cCaller 迁移映射（`migrate` 子命令输入草稿）

本文档是 `migrate` 子命令（M4，engineer 实现）的**字段级映射输入**：定义一份
hitest 风格配置如何机械地翻译成 cCaller 配置，以及哪些地方无法机械翻译、需要
人工介入或生成提示。对照样例见同目录 `hitest/` 与 `ccaller/`。

> 说明：cCaller 是 hitest 的重新设计版，配置模型更严格（`deny_unknown_fields`
> + 带行列号诊断 + 静态 slot def-use 分析），因此迁移不是纯改名，而是要补上
> hitest 靠约定隐式表达的语义。

---

## 1. ABI / wrapper 侧差异（先于配置迁移，wrapper 必须重编）

| 维度 | hitest | cCaller | 迁移动作 |
|---|---|---|---|
| 统一调用签名 | `s64 Call_x(u64 *param_page, const u64 *params, s64 len)` | `int64_t Call_x(uint64_t *param_page, const int64_t *params, int64_t len)` | `params` 由 `u64*` 改 `int64*`（有符号）；wrapper 重新编译 |
| 加载握手 | 无 | 必须导出 `int64_t CCaller_abi_version(void) -> 1`（FR-A-04） | wrapper 新增该符号 |
| 返回码分区 | 0=OK，-255=skip，其余负值=自定义失败（`-1/-12/-14` 随意） | 0=OK，-255=SKIP，`[-127,-1]`=自定义失败，`[-255,-128]`=框架保留（禁返），`>0`=保留 | 自定义失败码必须收敛进 `[-127,-1]`；越界负值会被判 contract violation |
| 字符串参数 | 单次调用内有效 | 同（Q-07），wrapper 需自持拷贝 | 无变化 |
| param_page | 512×u64，线程私有 | 同（`CCALLER_PARAM_PAGE_SLOTS`=512） | 无变化 |

## 2. 库描述 `libs.toml`

| hitest | cCaller |
|---|---|
| 无 `version` | `version = 1`（必填） |
| `[[libs]] path / funcs[{name, paras}]` | `[[libs]] path / funcs[{name, paras, slot_roles}]` |

- **`slot_roles`（新增，关键）**：hitest 不标注 slot 读写，靠参数命名约定 + wrapper
  宏（`GET_INPUT_IDX` 读 / `SET_OUTPUT_IDX` 写）隐式表达；cCaller 要求为每个
  “参数值是 param_page 下标”的参数显式标注 `read` / `write` / `read_write`
  （FR-C-10，驱动加载期 def-use 静态分析）。
- **迁移动作**：读 wrapper 源码，`GET_INPUT_IDX*`/`GET_VALUE` 取地址下标的标
  `read`，`SET_OUTPUT_IDX` 写下标标 `write`，两者皆有的标 `read_write`；纯传值
  参数（`GET_VALUE` 的普通数字/字符串）不标。**无法从 hitest 配置本身推断。**

## 3. 用例配置 `cases.toml` 顶层

| hitest | cCaller | 迁移动作 |
|---|---|---|
| 无 `version` | `version = 1`（必填） | 补 `version = 1` |
| `debug_test = "name"`（单个 String） | `debug_test = ["name"]`（Vec） | 单名包成单元素列表 |
| `default_serial`（bool） | `default_serial`（bool） | 同名 |
| `concurrences = [{tests, name}]` | `[[concurrences]]{name, tests}` | 语法改为数组-of-table；成员名必须存在 |
| `shared_inputs`（HashMap） | `shared_inputs`（BTreeMap） | 同语义；值见 §7 |
| `envs[]` 中 `tests = []` 表示全局 env | 独立的 `[env]` 表 | 全局 env 移到 `[env]` |
| `envs[]`（命名、含 tests） | `[[envs]]{name, init, exit, tests}` | 同名 |
| `thread_env` / `process_env`（各 `Option<Env>`） | `[thread_env]` / `[process_env]` | 同名 |

## 4. `Test`

| hitest | cCaller |
|---|---|
| `name`、`cmds` | 同 |
| `thread_num`（i64，缺省 1） | `thread_num`（u64，缺省 1） |
| `should_panic`（bool） | 同（M3 起子进程隔离） |
| `break_if_fail`（bool，缺省 true） | 同 |
| `serial`（`Option<bool>`） | 同 |
| `ref_inputs`（README 提及、源码未实现：cmds 头尾追加 init/cleanup） | **无对应** → 用 `env`/`envs` 层表达，迁移需提示 |

## 5. `Cmd` 与断言

| hitest | cCaller |
|---|---|
| `opfunc`、`args`、`perf` | 同（`perf=true` 计时，M3 起进报告） |
| 断言仅 `expect_eq` / `expect_ne`（二选一必填，`condition.rs` 互斥） | `expect_eq/ne/ge`（M3）+ `expect_lt/gt/le`（M4），开放命名空间；测试 Cmd 必须恰好一个已注册断言；env Cmd 可不带断言 |

**断言映射表：**

| hitest | cCaller |
|---|---|
| `expect_eq = X` | `expect_eq = X` |
| `expect_ne = X` | `expect_ne = X` |
| `expect_eq = "!X"`（hitest 的否定：`!=`） | `expect_ne = X`（或 `expect_eq = "!X"`，FR-C-07 自动折叠） |
| （无） | `expect_ge/expect_lt/expect_gt/expect_le`（新增，`!value` 折叠 ge↔lt 等） |
| （无） | `$!name`（变量否定，expect 位置专用） |

**args 值语法**：十进制、`0x` 十六进制、单引号字符串、`$var` —— 两版一致；cCaller
额外允许 `!`/`$!`，但只在 expect 位置合法（参数位置拒绝）。

## 6. `Env`

- hitest：`Env { name, init, exit, tests }`；全局 env = `envs[]` 里 `tests = []`（至多一个）。
- cCaller：`[env]`（全局，`name` 可选）、`[[envs]]`（`name` 必填）、`[thread_env]`、`[process_env]`；`init`/`exit` 都是 Cmd 列表。
- 迁移：`envs[]` 中 `tests=[]` 条目 → `[env]`；其余 → `[[envs]]`。

## 7. `InputGroup` 与输入展开

| hitest | cCaller |
|---|---|
| `name` 可省略（自动 `default1/default2…`） | `name` **必填** |
| `refs`（shared_inputs 复用） | 同 |
| `args`：`Single(String)` / `List([String])` / `Range{start:i32, end:i32, step:Option<i32>}`（step 缺省 1） | `args`：`Single(Int\|Str)` / `List` / `Range{start, end, step}`（step **必填**且 >0，闭区间） |
| 组级 `should_panic` / `break_if_fail`（覆盖 Test 级） | **无**（M4 计划补“组级覆盖”）→ 迁移需提示 |
| `$var` 用在 cmd args/expect | 同；且 own-args 值可 `$var` 引用**单值** shared 参数（FR-C-06） |

**子用例命名**：hitest 未严格规定；cCaller 用 Q-05 `{test}/{group}#{index}[k=v,…]`
（键按参数名排序，第一排序参数变化最慢）——见 `../expand/`。

## 8. skip / 返回码交互

| hitest | cCaller |
|---|---|
| 返回 `-255` → 整个 case 记为 skipped | 返回 `-255` → 该 Cmd 记为 skipped，case 继续，不计失败、不影响退出码 |

## 9. 无法机械迁移的点（migrate 必须提示/人工）

1. **`slot_roles`**：hitest 配置里没有读写标注，必须读 wrapper 源码推断。
2. **自定义失败码**：hitest 可返回任意负值；cCaller 要求收敛到 `[-127,-1]`。
3. **InputGroup 缺省名**：`default1/default2…` 需生成稳定的显式名（cCaller `name` 必填）。
4. **Range step**：hitest 缺省 → 显式 `step = 1`。
5. **组级 `should_panic` / `break_if_fail`**：cCaller 暂无对应，需提示或等待 M4 组级覆盖。
6. **`ref_inputs`**：hitest 专有，cCaller 用 env 层替代。
7. **`debug_test`**：单名 → 列表。
8. **`params` 有符号化**：wrapper 侧改动，配置侧无感知，但需提醒重编。

## 10. 对照样例

- hitest 风格：`hitest/libs.toml` + `hitest/cases.toml`（仅示意，ccaller 无法直接加载）
- cCaller 等价：`ccaller/libs.toml` + `ccaller/cases.toml`（`ccaller check` 应 0 findings）
