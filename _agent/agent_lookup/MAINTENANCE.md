memory-schema: v2

# agent_lookup — 长期维护纪律（写文件 / 维护时查这个；开工不必整读）

> 本文件 = 「每个文件怎么维护更新」的纪律，是这套系统两年不乱的根基。
> 开工操作见 `INDEX.md`；本文件管「怎么写才不乱、乱了怎么发现、人多久对一次账」。

## 一、每个文件的维护纪律（核心）


| 文件                                   | 谁写               | 何时动                      | 怎么动                         | 硬规则                                                       | 腐化迹象（该查什么）                   |
| -------------------------------------- | ------------------ | --------------------------- | ------------------------------ | ------------------------------------------------------------ | -------------------------------------- |
| `INDEX.md`                             | 人（Agent 可提议） | 改流程 / 加红线             | 直接编辑                       | 只放规则，不放任何条目/目录                                  | 出现具体条目或清单                     |
| `MAINTENANCE.md`                       | 人（Agent 可提议） | 规则变了                    | 直接编辑                       | 每条规则可执行、可被 check 校验                              | 规则与`memory.sh check` 不一致         |
| `project.md`                           | Agent 起草+人审    | 架构/接口/约束真变了        | 整节替换 + 更新该节「验证于」  | 不追加（会成流水账）；可从代码推导的不写                     | 出现操作步骤；日期戳久未动             |
| `status.md`                            | Agent              | 每次开工重写【现在】        | 覆盖写（tmp+rename 原子替换）  | 不追加；【现在】≤20 行                                      | 【现在】开始记历史流水                 |
| `short_term.md`                        | Agent              | 会话内完成小步              | 只追加一行指针                 | 不复述正文；≤40 条；按「工作线被取代」裁剪                  | 复述正文、只增不剪                     |
| `environment.md`                       | Agent 起草+人审    | 换工具链/rustc/依赖          | 整节替换 + 更新「验证于」      | 旧环境戳标「已停用」别删（要追溯旧决策的适用域）；不写密钥值 | 环境戳与实际不符                       |
| `errata.md`                            | Agent 起草+人审    | 确认 errata/workaround/修复 | 加一行；B 版修复确认后删对应行 | 每条带「验证于」+「复检触发」                                | workaround 长期「待验证」              |
| `decisions/`                           | Agent 起草+人审    | 拍板时                      | 一文件一决策；正文只增不删     | ≤10 行；推翻=新决策`superseded_by` 旧（不删旧）             | active 互相矛盾、>10 行                |
| `incidents/`                           | Agent 起草+人审    | bug 修复验证通过时          | 一文件一事故；正文只增不删     | 八字段齐全                                                   | 缺字段、症状词不在词表                 |
| `CATALOG.md`                           | 脚本（gen）        | 开工                        | 重建                           | 永不手改                                                     | 被人手改过                             |
| `tools/memory.sh` + `hooks/pre-commit` | 人                 | 规则变 / 发现 bug           | 编辑后重跑 check + 自测        | 改动后在真实提交里验证 hook                                  | check 报错没人管、`--no-verify` 成习惯 |

## 二、命名（decisions/ incidents/）

- `YYYY-MM-DD-<域>-<症状|主题>.md`（ASCII；同日多篇加 `-2`）；域取自词表；**归档后不改名 / 不移动 / 不建年目录**。

## 三、frontmatter（≤8 字段，平铺可 grep，不用 tags）

```
---
date: 2026-09-22
domain: concurrency
symptoms: [hang, 卡死]                        # incident 必填
status: draft                                # decision: draft|active|superseded|rejected；incident: draft|open|workaround|resolved|rejected
superseded_by: 2026-09-30-concurrency-par-map.md   # 被取代时填（引文件名）
scope: all / 全平台                           # 适用范围，必填
overrule_if: 压测证明 rayon 更优时重评        # decision 必填
related: []                                  # 双链，可空
---
```

- freshness 用内联 `（验证于 YYYY-MM-DD）`（project/environment 各节尾、errata 每条尾）；status 缺省一律 `draft`。

## 四、受控词表（封闭集；加词 = 改本文件）

- 域(12)：ffi config validate expand defuse run plan report assertion cli env tool
- 症状(15，中英双写)：timeout 超时 / hang 卡死 / crash 崩溃 / deadlock 死锁 / leak 泄漏 / oob 越界 / garbage 数据错乱 / misalign 未对齐 / flaky 偶发 / regression 回归 / initfail 初始化失败 / perf 性能异常 / compat 兼容性 / panic / silent 静默失败

## 五、git 纪律

- 消息 `memory(<域>|<文件>): 摘要` 一次一事；提交用 `-- agent_lookup/` pathspec（禁 add -A / add . / commit -a）。
- 禁对 agent_lookup/ reset --hard / rebase / amend；密钥值绝不入记忆（只写「存在哪 / 找谁」）。
- 回滚：`log --follow` → `show` 预览 → `restore --source` → 新提交固化（优先向前修正）。

## 六、对账节奏（人必须守的两个点 + 一个习惯）

- **开工批审 ≤2min**：清【待审】队列，keep / 改 / drop，翻状态（draft→active/rejected…）。
- **月度 30s**：`memory.sh check` + 扫上表「腐化迹象」列 + 对 `status` 与 `git log -1` 的时差。
- **引用习惯**：引用任何慢变事实前，先 git log / 现场探测对照（记忆是缓存，repo 是真相）。

## 七、升级信号（任一命中即评估，先于事故报警）

N≥50 每次 gen ｜ N≥150 脚本查询接口 ｜ 月内 R6≥3 扩词表 ｜ status 与 git log 时差 >14 天连续两轮 ｜ 待审 ≥10 条且 >7 天 ｜ 常驻 >12KB 瘦身 ｜ 硬件组合 >2 维升矩阵 ｜ 多人共享→独立 git ｜ 工具换代重接钩子 ｜ 描述性长查询频发→才评估向量检索。

## 八、风险速查（无法根除，靠纪律 + 升级信号）

静默权威谎言→引用前 git log/现场探测 ｜ 检索失效→R6 ｜ 硬件轴漂移→scope+现场探测 ｜ 写回断裂→开工批审 ｜ 密钥入库→红线 ｜ 钩子失效→换工具重验 ｜ 换岗→交接+健康分。
