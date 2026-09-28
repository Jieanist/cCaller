# CLI 参考

```
ccaller [全局选项] [子命令]
```

## 全局选项

| 选项 | 说明 |
|---|---|
| `-t, --test <FILE>` | 用例配置（别名 `--test_case`） |
| `-i, --lib <FILE>` | 库描述（别名 `--input`） |
| `-l, --log <LEVEL>` | 日志级别 1-4 或级别名；`RUST_LOG` 优先 |
| `-d, --debug <NAME>` | 只跑匹配测试（可重复） |
| `-m, --max-thread <N>` | 并发上限（≥1） |
| `-h, --help` / `-V, --version` | 帮助 / 版本 |

## 子命令

| 子命令 | 说明 |
|---|---|
| `run`（默认） | 并发执行；`--format text\|json\|junit`；`--allow-empty` 允许 0 用例 |
| `check` | 校验 + 静态 def-use + 展开统计，不执行；`--format text\|json` |
| `expand` | 打印每个用例展开后的子用例清单（名字 + 绑定）；`--format text\|json` |
| `init [DIR]` | 脚手架生成 libs.toml + cases.toml；`--build-sh` 附带编译脚本；不覆盖已存在文件 |
| `fmt [FILE]` | 规范化重排配置；默认 stdout；`--in-place` 原位写回 |

> `fmt` 的原地写回只有长旗 `--in-place`——短旗 `-i` 已被全局 `-i/--lib` 占用（clap 冲突）。
> `fmt` 缺省 FILE 时回退用 `-t/--test` 指定的文件。

隐藏选项：`--isolate-test` / `--isolate-subcase`（成对校验，控制死亡测试隔离方式）。

## 退出码

| 码 | 含义 |
|---|---|
| 0 | 全部通过（允许 skipped） |
| 1 | 有用例失败（断言/env 失败）；0 用例且未 `--allow-empty` |
| 2 | 配置/环境/用法错误、库加载失败、debug 未命中 |
| 3 | 框架内部错误 |
