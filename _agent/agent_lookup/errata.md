# errata — 已知坑与跨版本 workaround 登记簿（一行一条活条目；不抄原文，只记编号 + 工程结论）

| 编号 | 模块 | 一句话 | scope | workaround 状态 | 复检触发 |
|---|---|---|---|---|---|
| E-01 | tool | 本机 rustc 1.85 < 项目 MSRV 1.98 | 本机开发环境 | 采用：装 1.98 工具链（验证于 2026-09-28） | 本机装 1.98 后移除 |
| E-02 | assertion | `expect_ge` 否定折叠到 `expect_lt` 但未注册 | 断言注册表 | 采用：`expect_ge="!7"` 视为 load-time error（有测试锁定）（验证于 2026-09-28） | 注册 expect_lt 后移除 |

（每条尾加 `（验证于 YYYY-MM-DD）`；确认修复后删除对应行）
