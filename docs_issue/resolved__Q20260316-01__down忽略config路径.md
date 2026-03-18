# 问题 ID
Q20260316-01

# 当前状态
resolved

# 最后更新时间
2026-03-17 14:24 +08:00

# 问题标题
`down` 命令忽略 `-c/--config` 指定的配置路径，导致无法停止非当前目录项目实例

# 问题摘要
down忽略config路径

# 问题描述
当用户通过 `-c/--config` 指向非当前目录下的 `onekey-tasks.yaml` 启动服务后，`down` 仍然只按执行命令时的当前目录读取 `.onekey-run/state.json`。这会导致相同的 `-c` 参数在 `up` 和 `down` 之间不对称：`up` 能正常把服务拉起到配置文件所在项目根目录，`down` 却无法定位同一实例，直接报“找不到 runtime state”。

# 严重程度
High

# 影响对象
CLI 命令路由、运行时状态持久化、所有使用 `-c/--config` 启动非当前目录项目的用户

# 问题原因
`up`/`check` 会基于 `cli.config` 构建 `RunPlan`，并把运行时状态写入配置文件所在目录；但 `down` 分支没有复用同样的定位逻辑，而是直接调用 `env::current_dir()` 作为 `project_root`。因此一旦配置文件不在当前工作目录，停止命令就会去错误目录找状态文件。

# 核心证据路径
`src/app.rs`

# 待确认差异
无

# 造成问题的证据
- 代码路径：
  - `src/app.rs:21-27` 中 `Command::Down` 直接使用 `env::current_dir()`
  - `src/orchestrator.rs:287-330` 中 `run_down` 仅按传入的 `project_root` 读取 `.onekey-run/state.json`
- 日志/报错：
  - 复现命令 `cargo run -- down -c tmp-review-config/onekey-tasks.yaml`
  - 输出 `failed to read runtime state /Users/wolf/RustroverProjects/onekey-run-rs/.onekey-run/state.json: No such file or directory (os error 2)`
- 配置位置：
  - `tmp-review-config/onekey-tasks.yaml`
- 复现步骤：
  1. 在仓库根目录执行 `cargo run -- init -c tmp-review-config/onekey-tasks.yaml`
  2. 执行 `cargo run -- up -c tmp-review-config/onekey-tasks.yaml`
  3. 另开终端，在同一仓库根目录执行 `cargo run -- down -c tmp-review-config/onekey-tasks.yaml`
  4. 观察到 `down` 未去 `tmp-review-config/.onekey-run/` 查找状态，而是错误读取仓库根目录下的 `.onekey-run/state.json`

# 影响
该问题会直接破坏 `-c/--config` 的核心使用场景，使多项目目录或集中管理配置路径的用户无法可靠停止已启动的服务。行为上表现为“启动成功但停止失败”，属于命令契约不一致，且会增加遗留进程与运行时文件残留风险。

# 建议解决方案
统一 `down` 与 `up/check` 的项目根目录解析方式。优先方案是让 `down` 也基于 `cli.config` 计算 `project_root`，再调用 `run_down`。同时补充一个覆盖“当前目录与配置文件目录不同”的回归测试，确保 `up -> down` 在同一路径参数下可闭环。

# 验收标准
1. 当执行 `onekey-run down -c <path/to/onekey-tasks.yaml>` 时，运行时状态目录必须按 `<config_dir>/.onekey-run/` 解析，而不是按命令执行时的当前目录解析。
2. 使用非当前目录配置文件执行 `up -c ...` 后，使用相同 `-c` 参数执行 `down -c ...` 必须能成功停止该实例并清理对应运行时状态文件。
3. 仓库中存在自动化测试或最小复现验证，覆盖“配置目录不等于当前目录”的 `down` 路径解析行为。

# 验证记录
- 标准 1 -> 验证动作：`cargo test app::tests::down_uses_config_parent_when_current_dir_differs` 与手工执行 `cargo run -- down -c tmp-review-config/onekey-tasks.yaml` | 结果：Pass | 证据/原因：自动化测试在“当前目录与配置目录不同”场景下通过；手工命令已按修复后逻辑读取 `tmp-review-config/.onekey-run/state.json`，不再回退到仓库根目录
- 标准 2 -> 验证动作：`cargo test app::tests::down_uses_config_parent_when_current_dir_differs` | 结果：Pass | 证据/原因：测试先在配置目录写入运行时状态，再以相同 `-c` 参数执行 `down`，结果成功返回并清理状态文件
- 标准 3 -> 验证动作：`cargo test` | 结果：Pass | 证据/原因：新增回归测试 `app::tests::down_uses_config_parent_when_current_dir_differs` 已纳入仓库自动化测试并随全量单测通过

# 评论
同意修复

# 状态变更记录
- 时间：2026-03-16 10:31 +08:00 | 状态：waiting_user | 原因：新建问题并等待用户确认 | 操作者：codex | 关联提交：N/A
- 时间：2026-03-17 14:15 +08:00 | 状态：approved | 原因：用户评论为“同意修复”，允许进入执行阶段 | 操作者：codex | 关联提交：N/A
- 时间：2026-03-17 14:15 +08:00 | 状态：in_progress | 原因：开始按建议方案修复 `down` 的配置路径解析逻辑 | 操作者：codex | 关联提交：N/A
- 时间：2026-03-17 14:24 +08:00 | 状态：verifying | 原因：代码修改完成，进入验收验证阶段 | 操作者：codex | 关联提交：N/A
- 时间：2026-03-17 14:24 +08:00 | 状态：resolved | 原因：验收标准全部通过，问题关闭 | 操作者：codex | 关联提交：N/A
