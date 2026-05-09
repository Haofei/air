下面是这套 **Agent IR / Agent Module System** 的完整需求总结。可以把它当成一个产品需求文档的初版。

# 架构共识

AIR 需要区分 agent 内部执行模型和 agent 之间的组合模型：

```text
Agent module 内部 = bounded state machine
Agent system / link layer = DAG / graph of modules
Planner / orchestration layer = dynamic RunPlan that selects and connects modules
```

原因是单个 agent 的真实执行通常是：

```text
observe state
decide next action
call model or tool
write result into state
transition phase
repeat until done / failed / step limit
```

这天然包含循环，例如：

```text
model -> tool -> model -> tool -> done
```

所以循环本身不是错误，**无界循环才是错误**。Agent module 的 verifier 应该检查：

```text
state schema
initial state
terminal states
max_steps
bounded retry
tool/model timeout
capability declaration
approval gate
state/output writes
```

DAG 仍然重要，但它属于外部组合层，用来 link 多个 module：

```text
github.read_pr -> review_agent -> security_agent -> report_agent
```

因此 AIR v0.1 的 workflow 应该支持两种形态：

```yaml
workflow:
  kind: state_machine
```

用于 agent module 内部执行。

```yaml
workflow:
  kind: dag
```

用于 system composition / module linking。

动态探索不应该塞进单个 `.air.yaml` 模块。新的分层是：

```text
高层 planner:
  - 接收任务
  - 查询 ModuleStore
  - 选择模块
  - 生成 RunPlan DAG
  - 根据中间结果重新规划

底层 AIR module:
  - 静态
  - 有界
  - 可验证
  - 权限封闭
  - 输入输出 schema 明确
```

`RunPlan` 是 planner 的输出，不是新的自由执行 runtime。AIR 会先把它 resolve 成系统 DAG，再做现有 linker/verifier 检查：

```text
module 是否存在
version 是否锁定
input/output schema 是否兼容
capability / policy 是否允许
是否有非法 cycle
是否缺少 required input
```

执行 trace 也分两层：

```text
$planner resolve_plan / decision
  -> module A trace
  -> module B trace
  -> final output
```

因此 AIR 的边界是：

```text
AIR Module = instruction set + module ABI + verifier unit
RunPlan = 单次动态组合计划
Planner = 负责为什么这样组合
Runtime/linker = 负责验证、执行、审计
```

最终用户接口应该是：

```text
air plan --task "..." --store company.air-store.yaml --model-config models.json --output run.air-plan.yaml
air run-plan run.air-plan.yaml --store company.air-store.yaml --input input.json
```

用户不需要手写 DAG。planner model 根据任务和 ModuleStore 生成 RunPlan，AIR 在写出计划前先验证：

```text
自然语言任务
  -> planner model
  -> RunPlan JSON/YAML
  -> AIR verifier
  -> run-plan
```

如果 planner 混淆 node edge 和 endpoint connect，CLI 可以做保守 normalization，但 normalization 后仍必须通过 verifier，不能绕过验证直接执行。

为了避免 planner 总是拿最小颗粒度模块重新组装，ModuleStore 不是简单文件列表，而是 capability catalog：

```yaml
modules:
  customer.triage@0.1.0:
    kind: composite
    visibility: public
    priority: 100
    covers: [extract_issue, score_risk, generate_report]

  customer.extract@0.1.0:
    kind: primitive
    visibility: internal
    priority: 10
```

规划策略：

```text
默认只把 visibility=public 的能力给 planner
优先选择 priority 高、covers 匹配的 composite
只有用户显式 --allow-internal，才暴露 primitive
即使 planner 选择 internal primitive，最终 RunPlan 仍必须通过 verifier
```

# 项目目标

做一个类似 **LLVM IR + Cargo + Helm + OpenTelemetry** 的 agent 工程系统。

它的目标不是再做一个 agent framework，而是定义一种中间表示，让 agent 可以被：

```text
生成
编译
链接
验证
部署
监控
回放
迁移
组合
```

最终效果是：

```text
同一个 agent / agent system
可以跑在 OpenAI、Claude、本地 runtime、LangGraph、Temporal、K8s、GitHub Actions、serverless 等不同平台上。
```

核心定位：

> **Composable Agent IR: compile, link, deploy, and observe agents across runtimes.**

---

# 一句话定义

这是一个面向 agent 的 **可组合、可验证、可部署、可观测的中间表示系统**。

它解决的问题是：

```text
现在每个平台都有自己的 agent harness
每个 agent 都绑定在自己的 runtime 上
agent 难迁移、难审计、难监控、难组合
multi-agent 系统大多靠自然语言聊天，工程稳定性差
```

所以需要一种 agent IR，让 agent 像软件模块一样被发布、组合和部署。

---

# 核心需求 1：Agent IR

需要定义一种标准格式，例如：

```text
.air.yaml
.air.json
```

它描述一个 agent 或 agent module 的完整结构。

至少包括：

```text
agent metadata
inputs
outputs
state
tools
model calls
workflow graph
policies
permissions
retry
timeout
approval gates
telemetry
deployment hints
runtime target
```

示例结构：

```yaml
agent:
  name: campaign-debug-agent
  version: 0.1.0

inputs:
  campaign_id: string
  environment: enum[prod, preprod]

outputs:
  root_cause: string
  evidence: list<object>
  recommended_action: string

tools:
  - name: snowflake.query
    capability: database.read

  - name: rails.console
    capability: internal.execute
    sandbox: readonly

workflow:
  - id: query_events
    tool: snowflake.query

  - id: analyze
    model: reasoning

  - id: report
    output: root_cause

policy:
  max_tool_calls: 30
  timeout_seconds: 600
  require_approval:
    - production.execute
    - database.write

observability:
  traces: true
  metrics:
    - tool_latency
    - token_usage
    - retry_count
```

---

# 核心需求 2：不是 prompt wrapper

这个系统不能只是把 prompt 包一层。

它必须表达 agent 的工程结构：

```text
typed input/output
explicit workflow graph
tool ABI
state transition
permission model
approval gate
failure handling
trace event
deployment contract
```

也就是说，它不是：

```text
prompt + tools
```

而是：

```text
agent program
```

---

# 核心需求 3：Typed Interface

所有 agent module 都必须有类型化输入输出。

例如：

```yaml
inputs:
  repo: string
  pr_number: integer

outputs:
  review_summary: string
  risk_score: number
  blocking_issues: list<object>
```

这样不同 agent 才能拼接。

编译器需要检查：

```text
Agent A 的 output 是否能传给 Agent B 的 input
字段类型是否匹配
required field 是否存在
schema 是否兼容
```

错误示例：

```text
Error:
deploy.canary requires risk_score: number,
but github.pr-review outputs risk_score: string.
```

---

# 核心需求 4：Tool ABI

需要定义统一 tool call 格式。

不同平台的 tool call 机制不同：

```text
OpenAI function call
Claude tool_use
LangGraph tool node
Temporal activity
local Rust trait
HTTP RPC
```

但 IR 里应该统一成一个 ABI：

```json
{
  "tool": "snowflake.query",
  "input": {},
  "capability": "database.read",
  "timeout_ms": 30000,
  "idempotency_key": "...",
  "trace_id": "..."
}
```

每个 backend adapter 再把它转换成目标平台格式。

---

# 核心需求 5：Capability / Permission Model

这是最重要的安全需求之一。

每个 module 必须声明自己需要什么权限：

```yaml
requires:
  capabilities:
    - github.read
    - github.comment
    - snowflake.read

denied:
  - git.push
  - production.deploy
  - secrets.read_all
```

系统安装、链接、部署前必须能看到权限清单：

```text
This module requests:
- github.read
- github.comment
- ci.run

This module does not request:
- git.push
- production.deploy
- secrets.read
```

权限变化必须被视为兼容性问题。

例如：

```text
read-only -> write
staging -> production
no approval -> approval optional
```

这些都应该是 breaking change。

---

# 核心需求 6：Approval Gate

生产写操作、危险操作、外部发送操作必须支持人工审批。

例如：

```yaml
policy:
  require_approval:
    - production.execute
    - database.write
    - email.send
    - k8s.rollback.production
```

编译器需要检查：

```text
如果某个 workflow path 会触发 production.write
那么路径中必须存在 approval gate
```

否则编译失败：

```text
Error:
production.rollback requires approval gate,
but no approval gate exists between monitor and rollback.
```

---

# 核心需求 7：Workflow Graph

Agent 不能只是自由聊天，应该是显式 workflow graph。

支持基本节点：

```text
model_call
tool_call
branch
loop
parallel
join
approval
emit_event
save_artifact
load_memory
return
```

示例：

```text
PR opened
   ↓
Fetch diff
   ├── Code review
   ├── Security scan
   └── Test generation
        ↓
Risk scoring
        ↓
Approval gate
        ↓
Deploy
        ↓
Monitor
        ↓
Rollback if needed
```

重点是：

```text
typed message passing
explicit dependency
bounded retry
observable trace
```

不要做：

```text
Agent A 和 Agent B 自由聊天
```

---

# 核心需求 8：Multi-Agent Linking

需要一个 Agent Linker。

它负责把多个 agent module 组合成一个系统。

输入：

```text
module A
module B
module C
connection config
```

输出：

```text
Composite Agent IR
```

Linker 需要做：

```text
连接 output -> input
检查 schema 兼容性
合并权限
合并 tool registry
合并 telemetry
发现循环依赖
发现危险路径
生成统一 deployment plan
```

例如：

```yaml
connect:
  github.pr-review.risk_score -> deploy.canary.risk_score
  qa.playwright.result -> deploy.canary.test_result
```

---

# 核心需求 9：Verifier

需要一个静态验证器，在运行前检查系统安全性和正确性。

Verifier 至少检查：

```text
schema mismatch
missing required input
unknown tool
missing permission
permission escalation
missing approval gate
infinite loop risk
unbounded retry
missing timeout
dangerous production path
secret leakage risk
unsupported backend feature
```

示例错误：

```text
Error:
node "fix_campaign" calls production.execute,
but no approval gate exists.
```

---

# 核心需求 10：Optimizer / Scheduler

多个 agent 一起编译后，可以做优化。

优化包括：

```text
并行执行
tool call 去重
cache 共享
公共输入复用
无用节点删除
失败路径裁剪
成本估算
latency 估算
```

例如三个 agent 都要读 PR diff：

```text
PR Review Agent reads diff
Security Agent reads diff
Test Agent reads diff
```

优化成：

```text
Fetch PR diff once
   ├── Review
   ├── Security
   └── Test
```

Scheduler 需要决定：

```text
哪些节点可以并行
哪些节点必须串行
哪些节点需要等待 approval
哪些节点失败后可以继续
哪些节点必须 fail fast
```

---

# 核心需求 11：Runtime Adapter

同一个 IR 应该可以编译到不同 runtime。

目标 backend 包括：

```text
local Rust runtime
OpenAI adapter
Claude adapter
LangGraph adapter
Temporal workflow
Kubernetes Job
GitHub Actions
serverless
browser worker
internal company platform
```

命令形式：

```bash
airc compile agent.air.yaml --target openai
airc compile agent.air.yaml --target claude
airc compile agent.air.yaml --target local
airc compile agent.air.yaml --target temporal
airc compile agent.air.yaml --target k8s
```

---

# 核心需求 12：Observability / Trace

所有 agent 执行必须产生标准 trace。

Trace 至少包括：

```text
run_id
step_id
agent_id
module_id
tool_call_id
input hash
output hash
model name
token usage
latency
retry count
failure reason
approval decision
artifact links
timestamp
```

需要支持：

```bash
air trace run-123
air replay run-123
air diff run-123 run-124
```

这对 production agent 很重要，因为可以回答：

```text
agent 看到了什么？
调用了什么 tool？
为什么做这个 decision？
哪一步失败？
是模型错了，还是工具返回错了？
能不能复现？
```

---

# 核心需求 13：Replay / Debug

需要支持回放。

回放分两种：

```text
deterministic replay
mock replay
```

Deterministic replay 使用保存的 tool output 和 model output。

Mock replay 使用 fake tool / fixture。

用途：

```text
debug 失败
比较版本差异
验证 agent module 升级是否安全
做 regression test
```

---

# 核心需求 14：Package Manager

需要类似 Cargo/npm 的 agent package manager。

命令示例：

```bash
air add github.pr-review
air add qa.playwright-test
air add deploy.k8s-canary
air add monitor.datadog
air add rollback.safe-rollback
```

Module package 需要包含：

```text
AIR spec
schemas
tool requirements
policies
tests
examples
README
version metadata
compatibility info
```

---

# 核心需求 15：Module Registry

需要 module registry。

类似：

```text
crates.io
npm
helm chart repo
terraform registry
```

每个 module 发布时需要包含：

```text
name
version
description
inputs
outputs
required capabilities
supported targets
security profile
license
publisher
checksum/signature
```

安装时需要展示：

```text
这个 module 要什么权限
支持哪些 runtime
是否有 production write
是否需要 approval
是否有已知安全风险
```

---

# 核心需求 16：Semantic Versioning

Agent module 需要 semver，但规则比普通代码更严格。

Breaking change 包括：

```text
input schema 改变
output schema 改变
required permission 增加
read permission 变 write permission
staging permission 变 production permission
approval gate 被移除
failure behavior 改变
telemetry event 改名
default side effect 改变
```

版本规则：

```text
major:
  schema / permission / behavior contract breaking change

minor:
  新增 optional input/output
  新增非必需能力
  新增 telemetry event

patch:
  bug fix
  prompt improvement
  performance improvement
  不改变 contract 的内部优化
```

---

# 核心需求 17：Security Model

因为 agent module 可能调用真实系统，所以安全要比 npm 更严格。

必须支持：

```text
capability-based security
least privilege
sandboxing
secret isolation
approval policy
audit log
module signing
dependency scanning
permission diff
supply-chain verification
```

安装或升级 module 时，必须显示 permission diff：

```text
v1.2.0 -> v1.3.0

New permission requested:
- github.comment

No production permission added.
```

危险升级：

```text
v1.3.0 -> v2.0.0

New dangerous permission:
- production.deploy

Approval required before install.
```

---

# 核心需求 18：Artifacts

Agent 执行会产生 artifact。

例如：

```text
report
screenshot
trace file
test result
log bundle
SQL result
deployment manifest
diff
patch
playwright trace
```

IR 需要定义 artifact ABI：

```yaml
artifacts:
  - name: investigation_report
    type: markdown
  - name: playwright_trace
    type: file
  - name: deployment_plan
    type: yaml
```

Artifact 需要能被后续 agent 消费。

---

# 核心需求 19：Memory / State

Agent 需要明确状态模型。

不能让每个 agent 自己乱存 memory。

需要支持：

```text
local state
shared state
persistent memory
ephemeral memory
artifact reference
event log
```

示例：

```yaml
shared_state:
  pr_summary: string
  test_result: object
  risk_score: number
  deployment_id: string

agents:
  pr_review:
    write: [pr_summary, risk_score]

  qa:
    read: [pr_summary]
    write: [test_result]

  deploy:
    read: [test_result, risk_score]
    write: [deployment_id]
```

---

# 核心需求 20：Target Feature Detection

不同 runtime 能力不一样。

比如：

```text
OpenAI 支持 hosted tools
Claude 有不同 tool-use 格式
Temporal 支持 durable workflow
K8s 适合 long-running job
GitHub Actions 适合 CI trigger
Browser Worker 不能访问本地文件
```

编译器需要检查 target 是否支持 module 需要的能力。

例如：

```text
Error:
module deploy.k8s requires durable approval wait,
but target github-actions does not support long-running approval state.
```

---

# 核心需求 21：CLI

需要一个 CLI，类似：

```bash
air init
air validate
air link
air compile
air run
air test
air deploy
air trace
air replay
air add
air publish
```

建议组件：

```text
airc      compiler / validator / linker
airrun    local executor
airpkg    package manager
airtrace  trace viewer / replay tool
airdeploy deployment tool
```

---

# 核心需求 22：Rust 实现模块划分

Rust 项目可以这样拆：

```text
air-parser
air-schema
air-ir
air-typecheck
air-linker
air-verifier
air-optimizer
air-scheduler
air-runtime
air-backend-openai
air-backend-claude
air-backend-local
air-backend-temporal
air-backend-k8s
air-package
air-registry-client
air-trace
air-cli
```

最关键的是：

```text
air-ir
air-typecheck
air-linker
air-verifier
air-runtime
```

最先做：

```text
air-parser
air-ir
air-verifier
air-runtime
air-cli
```

---

# MVP 范围

不要一开始做完整生态。

第一版只做一个能跑通的闭环。

## AIR v0.1 支持

```text
YAML/JSON module format
typed input/output
state schema
tool declaration
agent state_machine workflow
composition DAG
model_call
tool_call
branch
retry
timeout
approval gate
trace event
local runner
OpenAI adapter
Claude adapter
basic linker
basic verifier
```

## 第一批内置 module

```text
github.read_pr
github.comment_pr
openai.analyze
playwright.run_test
slack.report
```

## 第一个 demo

```text
PR opened
   ↓
read_pr
   ↓
analyze_diff
   ├── security_check
   └── generate_tests
   ↓
playwright_test
   ↓
slack_report
   ↓
github_comment
```

这个 demo 能证明：

```text
module 可以组合
schema 可以检查
权限可以声明
workflow 可以编译
执行可以 trace
结果可以 replay
```

---

# 长期愿景

长期可以变成：

```text
LLVM + Cargo + Kubernetes + OpenTelemetry for agents
```

或者：

```text
Agent OS
Agent package ecosystem
Composable automation platform
```

最终用户可以发布自己的 agent module：

```text
github.pr-review
jira.ticket-triage
salesforce.account-research
snowflake.data-check
k8s.canary-deploy
datadog.incident-monitor
slack.report
```

别人可以把这些模块拼起来，形成复杂自动化系统。

---

# 关键设计原则

最重要的原则是：

```text
1. Agent module 必须 typed。
2. Tool call 必须有 ABI。
3. Permission 必须显式声明。
4. Production side effect 必须可审计。
5. Approval gate 必须是 IR 的一等公民。
6. Trace 必须标准化。
7. Replay 必须支持。
8. Multi-agent 不能靠自由聊天。
9. Module composition 必须通过 typed input/output。
10. Runtime adapter 不能污染核心 IR。
```

最核心的一句话：

> 不要做 agent chatroom，要做 agent compiler。

---

# 最终需求总结

这个系统需要支持：

```text
Agent IR
Agent ABI
Tool ABI
Typed module interface
Capability security
Workflow graph
Multi-agent linker
Static verifier
Runtime scheduler
Backend compiler
Package manager
Module registry
Trace/replay/debug
Deployment adapter
Versioning and compatibility rules
```

它的价值是：

```text
让 agent 从 prompt demo 变成工程 artifact
让不同 agent 可以像软件模块一样组合
让复杂 agent 系统可以被编译、验证、部署和监控
让 agent 不再绑定单一平台
```

最小产品不是“行业标准”，而是：

> 一个 Rust 写的 AIR compiler + local runtime + module linker + OpenAI/Claude adapter。
