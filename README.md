# cCaller

通用 C 接口测试执行框架：用 TOML 描述被测动态库与测试用例，框架负责加载期校验、
输入展开、静态槽位 def-use 分析与执行调度。

> **当前状态**：里程碑 M1 —— 配置模型与 `ccaller check` 已可用。
> `run`（实际执行）在后续里程碑落地。规范见 `docs/v1.0/` 四件套（唯一权威）。

## 构建

需要 Rust 1.98+（MSRV）：

```
cargo build --release
```

产物为 `target/release/ccaller`（Windows 下为 `ccaller.exe`）。

## 五分钟上手

写两份 TOML：**库描述**（声明被测动态库与导出函数）与**用例配置**（声明测试与命令序列）。

```toml
# libs.toml —— 库描述（规范见需求说明书 7.2）
version = 1

[[libs]]
path = "wrapper.dll"   # .so（Linux）/ .dll（Windows），相对本文件
funcs = [
  { name = "Call_malloc", paras = ["len", "mem_idx"], slot_roles = { mem_idx = "write" } },
  { name = "Call_read32", paras = ["addr_idx"],      slot_roles = { addr_idx = "read" } },
]
```

```toml
# cases.toml —— 用例配置（规范见需求说明书 7.3）
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

机器可读输出（JSON 契约见需求说明书 7.6）：

```
ccaller --test cases.toml --lib libs.toml check --format json
```

## CLI

全局选项（与 `--help` 输出一致）：

| 选项 | 说明 |
|---|---|
| `-t, --test <FILE>` | 用例配置文件（需求说明书 7.3）；兼容别名 `--test_case` |
| `-i, --lib <FILE>` | 库描述文件（需求说明书 7.2）；兼容别名 `--input` |
| `-l, --log <LEVEL>` | 日志级别：1-4（error/warn/info/debug）或级别名；`RUST_LOG` 优先 |
| `-h, --help` | 打印帮助 |
| `-V, --version` | 打印版本 |

子命令：

| 子命令 | 说明 |
|---|---|
| `check` | 加载期校验 + 静态槽位 def-use 分析 + 展开数量统计，不执行；`--format text\|json`（默认 `text`） |

`run`（执行用例）等其余子命令见需求说明书 FR-X-01，随后续里程碑落地。

## 退出码

| 码 | 含义 |
|---|---|
| 0 | 全部通过（允许存在 skipped） |
| 1 | 存在用例失败（随 M2 `run` 落地） |
| 2 | 配置/环境错误：加载期校验失败、用法错误、文件不可读 |
| 3 | 框架内部错误（bug） |

契约出处：需求说明书 FR-X-02 / 7.5。

## 配置格式

字段表、值语法与完整示例的规范性定义见 `docs/v1.0/需求说明书.md` §7.2（库描述）
与 §7.3（用例配置）。README 只提供最小可跑示例，不复制字段表
（三类文档各司其职，见代码与注释风格规范 §12.1）。

## 仓库结构

| 目录 | 说明 |
|---|---|
| `crates/core` | 领域层：配置解析、校验、输入展开、def-use 分析 |
| `crates/cli` | `ccaller` 命令行 |
| `crates/ffi`、`crates/gen` | 预留（后续里程碑） |
| `docs/v1.0/` | 唯一权威规范：需求说明书 / FeatureList / 验证方案 / 代码与注释风格规范 |
| `verify/` | 验证脚本与语料（治理规则见验证方案 §5，`verify/` 冻结后只读） |
