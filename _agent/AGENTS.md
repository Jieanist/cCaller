# 项目 AI 协作约定（Agent 必读入口）

本项目：cCaller —— hitest 的重新设计版：通用 C 接口测试执行框架（TOML 描述被测动态库与用例，框架负责加载期校验、输入展开、静态槽位 def-use 分析与执行调度）。

每次开工，第一步：读 `agent_lookup/INDEX.md`，再按其中 R0~R6 检索配方取所需记忆。

写入纪律：记忆一律由你（Agent）起草，人只在开工批审时翻状态；未审条目标 `status: draft`，不要直接落 `active`/`open`。
