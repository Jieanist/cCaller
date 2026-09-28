# cCaller — agent_lookup 记忆系统

这是「agent_lookup 长短期记忆系统」在本项目（cCaller：hitest 的重新设计版，通用 C 接口测试执行框架）的落地。存储层 = 纯文本 markdown + git，运行时 = Agent 起草、人只审不写。

## 一句话

- **存储层** = 纯文本 markdown + git（不引数据库 / 向量库 / 独立服务）。
- **运行时** = 写入交给 Agent（人只审不写）、防腐靠 frontmatter 状态字段 + 生成式索引 + 对账。
- **人只留两个动作**：开工批审 ≤2 分钟 + 月度对账 30 秒。

## 快速开始（人只跑这 3 条验证）

```bash
cd /home/e0007816/nfs/cCaller/_agent
agent_lookup/tools/memory.sh install-hooks      # 装版本化 pre-commit hook
agent_lookup/tools/memory.sh gen                # 生成 CATALOG.md
agent_lookup/tools/memory.sh check              # 体检（应输出「全绿」）
```

## 开工固定动作（R0）

1. 读 `agent_lookup/INDEX.md`（协议）。
2. 跑 `memory.sh gen`（重建 CATALOG，R0 无条件执行）。
3. 读常驻 6 件 + CATALOG「现行法律」视图（active 决策 + open/workaround incident）。
4. Agent 报「上次到哪 / 今天建议 / 待审 N 条」，人在编译/测试等待窗批审 ≤2 分钟。

## 协议全文

见 `agent_lookup/INDEX.md`（身份/开工流程/写入触发/命名/frontmatter/受控词表/检索 R0~R6/git 纪律/密级红线/降级模式）。

## 维护者参考（人读，不进 Agent 开工上下文）

**升级触发（任一命中即评估，先于事故报警）**
N≥50 每次 gen ｜ N≥150 脚本查询接口 ｜ 月内 R6≥3 扩词表 ｜ status 与 git log 时差 >14 天连续两轮 ｜ 待审 ≥10 条且 >7 天 ｜ 常驻 >12KB 瘦身 ｜ 环境组合 >2 维升矩阵 ｜ 多人共享→独立 git ｜ 工具换代重接钩子 ｜ 描述性长查询频发→才评估向量检索。

**风险速查（无法根除，靠纪律 + 升级信号）**
静默权威谎言→引用前 git log/现场探测 ｜ 检索失效→R6 ｜ 环境轴漂移→scope+现场探测 ｜ 写回断裂→开工批审 ｜ 密钥入库→红线 ｜ 钩子失效→换工具重验 ｜ 换岗→交接+健康分。
