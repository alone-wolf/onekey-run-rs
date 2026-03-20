# 问题 ID
Q20260316-02

# 当前状态
resolved

# 最后更新时间
2026-03-20 11:55 +08:00

# 问题标题
`init` 模板和仓库示例配置默认使用 Unix 命令，和当前跨平台目标不一致

# 问题摘要
模板示例偏Unix

# 问题描述
当前 `init` 生成的模板使用 `sleep`，`init --full` 新增的 `actions` 示例继续大量使用 `sh -c`，仓库根目录示例 `onekey-tasks.yaml` 也仍然使用 shell 片段。这些内容在 Unix-like 环境下可运行，但在默认 Windows 环境中通常不可直接通过 `check` 或 `up`。在项目已经明确保留 Windows 支持目标的前提下，这会让新用户一上手就得到不可执行的示例配置，而且本轮 hooks/actions 的扩展让这种偏差比之前更明显。

# 严重程度
Medium

# 影响对象
`init` 命令、新用户上手体验、Windows 用户、示例文档与发布包

# 问题原因
模板和示例配置仍以 Unix-like 环境为默认假设，没有为 Windows 提供等价模板，也没有在命令输出或文档中显式声明“当前模板仅适用于 Unix-like”。因此代码层面的跨平台进程控制目标，与配置示例层面的平台假设发生了偏差。

# 核心证据路径
`src/orchestrator.rs`

# 待确认差异
无

# 造成问题的证据
- 代码路径：
  - `src/orchestrator.rs` 中 `INIT_TEMPLATE` 继续使用 `sleep`
  - `src/orchestrator.rs` 中 `INIT_TEMPLATE_FULL` 新增的 `prepare-app`、`notify-up`、`notify-stop`、`notify-exit` 全部使用 `sh`
- 日志/报错：
  - 在默认 Windows 环境中，`sleep` 与 `sh` 通常不在 PATH，`check` 将报 executable not found
- 配置位置：
  - `onekey-tasks.yaml:7-16` 和 `onekey-tasks.yaml:34-44` 使用 `sh -c` 多行 shell 片段
- 复现步骤：
  1. 在 Windows 环境执行 `onekey-run init`
  2. 直接执行 `onekey-run check`
  3. 预期会因模板中的 `sleep` 不存在而失败；仓库根目录示例同理会因 `sh` 不存在而失败

# 影响
该问题不会破坏所有运行场景，但会直接影响首轮体验和跨平台可信度。对 Windows 用户而言，工具虽然实现了部分平台兼容逻辑，默认模板和示例却无法直接通过最基础的 `check`，这会造成“产品宣称支持，但默认配置不可用”的认知落差。

# 建议解决方案
二选一明确化：要么为 `init` 与示例配置提供按平台生成的可运行模板，要么在当前阶段明确声明模板与仓库示例仅支持 Unix-like，并在文档中给出 Windows 对应示例。无论采用哪种方式，都应保证 `init` 产物与项目对外平台承诺一致。

# 验收标准
1. `init` 生成的默认模板在项目声明支持的平台上都具有明确可执行语义，或在不支持的平台上有显式限制说明。
2. 仓库根目录示例配置与平台支持文档保持一致，不再隐含“默认跨平台可用”但实际依赖 Unix shell 的矛盾状态。
3. 至少存在一条文档化验证路径，说明 Windows 用户应使用的示例配置或当前限制说明。

# 验证记录
- 标准 1 -> 验证动作：执行 `cargo test`，并新增/通过 `preset_minimal_windows_uses_cmd_timeout`、`preset_full_windows_uses_cmd_for_services_and_actions`、`preset_full_windows_round_trips_through_yaml` | 结果：Pass | 证据/原因：`src/config.rs` 已新增 Windows 版 minimal/full presets，`preset_minimal()` / `preset_full()` 会按当前平台选择对应模板
- 标准 2 -> 验证动作：检查 `src/config.rs` 的平台预设切换逻辑，并核对 `docs_dev/02_cli_contract.md` 中对仓库工作区示例与 `init` 生成模板的说明 | 结果：Pass | 证据/原因：`init` 已走平台 preset；文档已明确 Windows 用户应优先使用 `onekey-run init` 生成模板，而不是直接复用仓库工作区中的临时示例配置
- 标准 3 -> 验证动作：检查 `docs_dev/02_cli_contract.md` 与 `docs_dev/03_config_schema.md` | 结果：Pass | 证据/原因：文档已说明 `init` / `init --full` 按当前运行平台生成模板，Windows 用户有明确使用路径

# 评论
好的，按照你的规划执行

# 延期原因
此前曾延期处理，但本轮 `actions` / `hooks` 与 `init --full` 扩展继续增加 Unix-only 示例，问题已被新变更再次放大，需要重新等待用户确认是否纳入后续修复范围。

# 延期时间
2026-03-17 14:13 +08:00

# 下次复审时间
TBD

# 状态变更记录
- 时间：2026-03-16 10:31 +08:00 | 状态：waiting_user | 原因：新建问题并等待用户确认 | 操作者：codex | 关联提交：N/A
- 时间：2026-03-17 14:13 +08:00 | 状态：deferred | 原因：用户评论为“暂不处理”，按流程延期保留记录 | 操作者：codex | 关联提交：N/A
- 时间：2026-03-19 23:43 +08:00 | 状态：waiting_user | 原因：本轮 review 发现 `init --full` 新增 Unix-only action 示例，原问题再次出现并扩大影响范围，按流程复开等待用户确认 | 操作者：codex | 关联提交：N/A
- 时间：2026-03-20 11:45 +08:00 | 状态：approved | 原因：用户评论为“好的，按照你的规划执行”，确认按平台模板方案修复 | 操作者：codex | 关联提交：N/A
- 时间：2026-03-20 11:45 +08:00 | 状态：in_progress | 原因：开始实现 Windows 版 minimal/full 预设并接入当前模板生成路径 | 操作者：codex | 关联提交：N/A
- 时间：2026-03-20 11:55 +08:00 | 状态：verifying | 原因：Windows 版 presets、平台切换逻辑和说明文档已完成，开始按验收标准验证 | 操作者：codex | 关联提交：N/A
- 时间：2026-03-20 11:55 +08:00 | 状态：resolved | 原因：验收标准全部通过，`init` 模板已具备按平台生成 Windows 对应 preset 的能力，并补充了文档化使用路径 | 操作者：codex | 关联提交：N/A
- 时间：2026-03-16 10:31 +08:00 | 状态： | 原因： | 操作者： | 关联提交：
