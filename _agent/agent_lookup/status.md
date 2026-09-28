# status — 当前状态（每次开工 Agent 重写【现在】；覆盖写用 tmp+rename 原子替换）

## 【现在】（≤20 行；重写，不追加）
- 版本锚：`20381ad`（cCaller main HEAD，验证于 2026-09-28）
- 工作线：hitest → cCaller 重新设计；M0(骨架)/M1(配置模型+check)/M2(串行 run+四层 env+退出码) 已完成
- 阻塞：无
- 今日建议：补齐 M3 里程碑 —— 优先级 并发执行 > 死亡测试隔离 > perf 计时 > debug 过滤 > JSON/JUnit 报告 > wrapper 宏/生成器
- 已知缺口（详见 project.md「未实现」）：thread_num/concurrences/max-thread 未接执行器；should_panic 直接 skip；perf 未计时

## 【待审】（Agent 维护；人开工清空 keep/改/drop）
- （暂无 draft 条目）
