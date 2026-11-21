# AgentConsole 使用说明

AgentConsole 在原生 Codex TUI 中管理一个 main agent 和多个 component agent。
Main 通过 `delegate_task` 分配任务；你也可以切换到某个 component，直接与它对话。

## 从源码启动

需要 Node.js 22 或更新版本，以及交互式终端。目前控制通道使用 Unix socket，适用于 macOS / Linux。
以下命令从包含 `codex-cli`、`codex-rs` 的仓库根目录运行。

先构建此分支的原生 Codex 客户端：

```sh
cd codex-rs
cargo build -p codex-cli
cd ..
```

首次使用时，先登录：

```sh
./codex-rs/target/debug/codex login
```

启动通用示例或 kernel development 配置，二选一：

```sh
node ./codex-cli/bin/agentconsole.js ./codex-cli/agentconsole.example.json
```

```sh
node ./codex-cli/bin/agentconsole.js ./codex-cli/agentconsole.kernel.json
```

如果安装的包已提供 `agentconsole` 命令，也可以直接运行：

```sh
agentconsole /absolute/path/to/config.json
```

**Agent 的工作目录是启动命令时的当前目录，不是配置文件所在目录。**
例如，使用本仓库的配置处理另一个项目：

```sh
cd /path/to/your/project
node ~/Dev/codex/.local/agentconsole/codex-cli/bin/agentconsole.js \
  ~/Dev/codex/.local/agentconsole/codex-cli/agentconsole.kernel.json
```

## 选择原生客户端

启动器按以下顺序选择二进制：

1. 环境变量 `CODEX_BIN` 指定的程序。
2. 本仓库的 `codex-rs/target/debug/codex`。
3. 包内的 `codex-cli/bin/codex.js` 启动包装器。

使用 release 构建或其他构建路径时，显式指定支持本分支 AgentConsole 功能的客户端：

```sh
CODEX_BIN=/absolute/path/to/codex \
  node ./codex-cli/bin/agentconsole.js ./codex-cli/agentconsole.kernel.json
```

## 配置模型与 effort

配置文件为 JSON，至少定义一个 component。例如：

```json
{
  "effort": "high",
  "main": {
    "model": "gpt-6-astra",
    "instructions": "Coordinate engineering tasks. Use delegate_task to assign work to implementer and reviewer."
  },
  "components": {
    "implementer": {
      "model": "gpt-6-astra",
      "effort": "medium",
      "instructions": "Implement requested changes and verify their behavior."
    },
    "reviewer": {
      "model": "gpt-6-astra",
      "effort": "medium",
      "instructions": "Review changes for correctness and report actionable findings."
    }
  },
  "observer": {
    "model": "gpt-5.6-sol"
  }
}
```

- 顶层 `effort` 是所有 agent（含 observer）的默认值。
- `main.effort`、`components.<name>.effort` 覆盖顶层默认值；上例中 main 使用 `high`，implementer 和 reviewer 使用 `medium`。
- 顶层 `effort` 是默认值，不是上限；本 kernel 测试配置默认使用 `high`。
- 顶层和角色均未设置 effort 时，使用服务端原有默认值。
- 启动器接受 `none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max`、`ultra`；具体可用值取决于模型和服务端。
- 目前 observer 没有独立的 effort 覆盖字段，继承顶层默认值。`observer.model` 可单独设置；未设置时继承当前显示的模型。
- Main / component 未设置 `model` 时使用服务端默认模型。示例模型需要你的账号和服务端支持。
- Component 名称只能包含字母、数字、下划线或连字符，不能叫 `main`。新增角色时，在 `components` 中添加对象，并更新 main 的分工说明。

修改配置后，退出并重新启动 AgentConsole 生效。

## Kernel development 角色

源码中的 `agentconsole.kernel.json` 提供以下分工：

| 角色 | 职责 |
| --- | --- |
| main | 协调任务、整合结果 |
| implementer | 实现 kernel 和优化 |
| reviewer | 代码审查 |
| profiler | 性能基线、profiling 和 benchmark |
| analyzer | 分析瓶颈及根因，提出实验 |
| checker | 独立验证正确性 |
| resource_manager | 协调计算资源和任务占用 |
| observer | 围绕选中的历史文本进行讨论 |

## 常用操作

以下快捷键均先按 `Ctrl+X`，松开，再按下一个键：

| 操作 | 快捷键 |
| --- | --- |
| 切换到下一个 agent | `Ctrl+X` → `N` |
| 查看当前 agent 的 Trace | `Ctrl+X` → `T` |
| 查看当前 agent 的任务队列 | `Ctrl+X` → `Q` |
| 选中聊天文本后创建 observer | `Ctrl+X` → `O` |
| 从 observer 返回来源 agent | `Ctrl+X` → `O` |

也可以点击 agent 切换对话。在 Trace 中选中文本后按 `d`，可创建 observer。
按 `Esc` 关闭 Trace / Queue，返回聊天。

查看启动帮助：

```sh
node ./codex-cli/bin/agentconsole.js --help
```
