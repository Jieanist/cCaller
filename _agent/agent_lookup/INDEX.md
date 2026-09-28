memory-schema: v2

# agent_lookup — 开工操作手册（每次会话开始读这个）

**你是谁**：本项目的 AI 协作者。这个目录是项目记忆（markdown + git）。职责：开工时按「开工流程」取上下文并**向用户汇报**；工作中把「拍板 / 复盘」起草成 draft 入库。**你写、人审**——没审过的一律标 `draft`，不得直接落 `active`/`open`。

## 开工流程（照做，无状态也能执行）
1. 读本文件。
2. 跑 `tools/memory.sh gen`（重建 CATALOG）。
3. 读 `project.md`（项目是干嘛的）→ `status.md`【现在】（上次到哪）→ `environment.md` + `errata.md`（当前工具链/环境戳与已知坑）。
4. 读 `CATALOG.md`「现行法律」视图（active 决策 + open/workaround incident）。
5. **向用户汇报四件事**：①上次做到哪 ②今天建议做啥 ③待审 N 条等你批 ④有无 >90 天未验证的过期旗标。

## 写入触发（写进哪、标什么；「怎么维护」的细节见 MAINTENANCE.md）
| 触发时刻 | 落点 | 状态 |
|---|---|---|
| 用户拍板一个技术决定 | `decisions/YYYY-MM-DD-<域>-<主题>.md` | `draft` |
| 一个 bug **修复验证通过** | `incidents/YYYY-MM-DD-<域>-<症状>.md` | `draft` |
| 每次开工 | 重写 `status.md`【现在】 | — |
| 会话内完成小步 | `short_term.md` 追加一行指针 | — |
| 换工具链 / rustc / 依赖 | `environment.md` | — |
| 确认一个 errata / workaround | `errata.md` | — |
| 一条决策被推翻 | 新决策 `superseded_by` 旧 | 旧→`superseded` |

> 每写一条 draft 后：在 `status.md`【待审】加一行 → 立即 commit：`git commit -m "memory(decision|incident): 摘要" -- agent_lookup/`。

## 工作时的检索（R1~R6；R0 = 上面的开工流程）
- **R1 定域** → **R2 标题层** `ls agent_lookup/{decisions,incidents} | grep <域>` → **R3 元数据层**（读前 10 行判定）→ **R4 正文层**（读全文沿 `related` 扩展）→ **R5 换词兜底**（中↔英，查 MAINTENANCE 词表）。
- **R6 失败反馈**：两次落空且耗时 >1h → 在 **「修复验证通过那一刻」** 采集 incident（勿拖到会话收尾）。

## 工具
- `tools/memory.sh gen`（开工必跑）｜`check`（体检）｜`install-hooks`（装版本化 hook）。

## 红线（细节在 MAINTENANCE.md）
- 密钥 / 密码 / token / 内网路径 / IP 值**不写**，只写「存放在哪 / 找谁」。
- 写文件、命名、frontmatter、git 提交的具体规则，一律查 **`MAINTENANCE.md`**。

## 降级模式
防腐失效（hook 未装 / check 报错 / 队列积压）→ 降为**只读 grep 档案**，止损优先，不整库推倒。
