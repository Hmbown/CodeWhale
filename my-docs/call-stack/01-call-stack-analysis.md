# Issue #3915 调用栈分析

## 两条路径，两个 Bug

### 路径 A: `$echo hello world` — 参数传入但被丢弃

```
用户输入: "$echo hello world"
    │
    ▼
commands/mod.rs:109   execute("$echo hello world", app)
    │
    ▼
commands/mod.rs:114-126  检测 $ 前缀 → 剥离 → 空格拆分
    skill_input = "echo hello world"
    parts = ["echo", "hello world"]
    skill_name = "echo"               ← 名字正确
    arg = Some("hello world")         ← 参数存在！
    │
    ▼
commands/mod.rs:127  run_skill_by_name(app, "echo", Some("hello world"))
    │
    ▼
skills.rs:288-298   ╔══════════════════════════════════════════╗
                    ║ pub fn run_skill_by_name(                ║
                    ║     app, name: "echo",                   ║
                    ║     _arg: Some("hello world")  ← 注意！ ║
                    ║ ) {                                      ║
                    ║     activate_skill(app, "echo");         ║
                    ║     // _arg 被标记为未使用               ║
                    ║     // "hello world" 在此丢失 ❌          ║
                    ║ }                                        ║
                    ╚══════════════════════════════════════════╝
    │
    ▼
skills.rs:326-347   activate_skill(app, "echo")
    app.active_skill = Some(instruction_str)
    返回 "Skill 'echo' activated. Type your request..."
    │
    ▼
用户看到: skill 激活了，但 "hello world" 消失了
用户必须重新输入一遍
```

**根因**: `skills.rs:290` 的参数名是 `_arg`（下划线前缀），编译器会忽略。
调用者 `mod.rs:127` 明明传了 `arg`，但函数内部完全不使用它。

---

### 路径 B: `/skill echo hello world` — 名字拆出来了但没用

```
用户输入: "/skill echo hello world"
    │
    ▼
commands/mod.rs:109   execute("/skill echo hello world", app)
    │  没有 $ 前缀，走普通 command 路径
    │
    ▼
commands/mod.rs:135-145  按空格拆分
    command = "skill"               ← 去掉 / 前缀
    arg = Some("echo hello world")  ← 整个剩余部分作为 arg
    │
    ▼
commands/mod.rs:173-174  注册表查找 "skill" → SkillCmd::execute(app, arg)
    │
    ▼
skills.rs:300  run_skill(app, Some("echo hello world"))
    raw = "echo hello world"
    │
    ▼
skills.rs:312-313  splitn(2, whitespace)
    head = "echo"
    rest = "hello world"    ← 名字和参数正确拆开了！
    │
    ▼
skills.rs:323  activate_skill(app, raw)   ╔═══════════════════════╗
                              └── 传入整个 raw！                 ║
                                  应该是 activate_skill(app, head) ║
                                  但实际传了 "echo hello world"   ║
                                  查不到这个 skill → 报错 ❌      ║
                                           ╚═══════════════════════╝
    │
    ▼
返回: "Skill 'echo hello world' not found."
```

**根因**: `skills.rs:323` 应该传 `head`（已拆出的名字），实际传了 `raw`（完整字符串）。

---

### 对照: `$echo`（无参）— 正常流程

```
用户输入: "$echo"
    │  → skill_name = "echo", arg = None
    ▼
run_skill_by_name(app, "echo", None) → activate_skill(app, "echo")
    app.active_skill = Some(instruction)
    │
用户下一条消息: "hello world" + 回车
    │
    ▼
ui.rs:6840-6842  build_queued_message()
    skill_instruction = app.active_skill.take()
    QueuedMessage::new("hello world", skill_instruction)
    │
    ▼
AI 收到: [skill_instruction] + "hello world" → 正确输出 ✅
```

---

## 修复方案

### 修复 A: `run_skill_by_name`
当 `arg` 不为空时，激活 skill 后立即将 arg 文本作为 `QueuedMessage` 排队。

### 修复 B: `run_skill`
`activate_skill(app, raw)` → `activate_skill(app, head)`
然后如果 `rest` 不为空，将其排队。

### 关键机制
`QueuedMessage` 有两个字段:
- `display: String` — 显示给用户的文本
- `skill_instruction: Option<String>` — skill 的 system prompt

`app.queue_message()` 将消息加入队列，`app.active_skill.take()` 取出 skill instruction。
当用户点击回车时，`ui.rs:6840-6842` 就是这个逻辑——我们的修复就是把这个逻辑提前到 skill 激活时执行。