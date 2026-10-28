# 输入框的键盘类型和回车键 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 输入框能指定键盘类型和回车键；Android 输入法按它显示键盘和动作键；"下一项"跳焦点，显式的完成/搜索/前往收起键盘。

**Architecture:** core 的 `InputData` / `EditorState` / `ImeTextState` 带上 `keyboard` 与解析后的 `enter_key`，编辑器按解析值处理回车；shell 用一个可在主机测试的纯函数把它们映射成 EditorInfo 数值，`sync_ime_text` 在映射结果变化时推送。

**Tech Stack:** Rust；fenestra-core、fenestra-shell（android-activity 0.6.1 GameActivity）、fenestra-kit；wechat-ui。

**Spec:** `docs/superpowers/specs/2026-10-28-keyboard-type-and-enter-key-design.md`

## 全局约束

- 没设置这两项的字段行为不变（单行 Done、多行换行、密码不建议不学习、`submit_on_enter` 照旧）。
- EditorInfo 只在映射结果变化时推送，且总在 `set_text_input_state` 之前。
- 共享 `CARGO_TARGET_DIR`；小改动按影响范围检查；推送前全量门控；wechat-ui 只本地提交。

## 审查重点

1. 拼音组合中按回车（选字）：不跳焦点、不失焦、不发送（Task 1 测试 `enter_while_composing_neither_moves_nor_blurs`）。
2. 显式值与 `submit_on_enter` 冲突：以显式值为准（Task 1 测试 `an_explicit_enter_key_overrides_submit_on_enter`）。
3. 焦点在两个不同类型字段间切换：输入法收到新的编辑器信息（Task 2 测试 `a_field_switch_repushes_editor_info`）。
4. 默认字段：映射结果与现在完全相同（Task 2 测试 `the_default_field_maps_as_before`）。
5. 最后一个"下一项"字段：绕回第一个，与 Tab 一致（Task 1 测试 `next_on_the_last_field_wraps_like_tab`）。

---

### Task 1: core 的类型、解析、回车行为

**Files:** `fenestra-core/src/element.rs`、`input.rs`、`frame.rs`、`frame_state.rs`、`events.rs`、`lib.rs`；测试 `fenestra-core/tests/enter_key.rs`（新建）。

**Interfaces（Produces）:**
- `#[non_exhaustive] pub enum Keyboard { #[default] Text, Number, Decimal, Phone, Email, Url }`
- `#[non_exhaustive] pub enum EnterKey { #[default] Default, Send, Search, Go, Done, Next, Newline }`
- `Element::keyboard(self, Keyboard) -> Self`、`Element::enter_key(self, EnterKey) -> Self`（只对输入元素生效）。
- `InputData { keyboard, enter_key, .. }`；`ImeTextState { keyboard, enter_key /* 解析后 */, multiline, .. }`。
- `pub fn resolve_enter_key(declared: EnterKey, multiline: bool, submit_on_enter: bool) -> EnterKey`。

- [ ] 写失败的测试：
  - `resolve_defaults_to_todays_behaviour`：`(Default, false, _) → Done`，`(Default, true, false) → Newline`，`(Default, true, true) → Done`，`(Newline, false, _) → Done`，`(Send, true, false) → Send`。
  - `ime_state_carries_the_field_kind`：聚焦 `raw_input(..).keyboard(Email).enter_key(Next)` 的 `ime_text_state()` 带 `Email`、`Next`、`multiline=false`。
  - `an_explicit_enter_key_overrides_submit_on_enter`：多行 + `submit_on_enter(true)` + `Newline`：回车插入换行（值变化）；多行无 `submit_on_enter` + `Send`：回车不改值，`on_key` 收到回车。
  - `next_moves_focus_to_the_next_field`：两个字段，第一个 `Next`，回车后焦点在第二个。
  - `on_key_takes_enter_before_next`：`Next` 字段的 `on_key` 返回消息时焦点不动。
  - `next_on_the_last_field_wraps_like_tab`。
  - `an_explicit_search_blurs_when_unhandled`：单行 `Search`，回车后无焦点；有 `on_key` 处理时焦点保留；默认单行字段回车焦点保留；`Send` 单行回车焦点保留。
  - `enter_while_composing_neither_moves_nor_blurs`：`Next` 和 `Search` 字段在组合中收到回车，焦点不变。
- [ ] 运行确认失败（编译失败）：`cargo test -p fenestra-core --test enter_key`。
- [ ] 实现：枚举与构建方法；`EditorState` 存解析后的 `enter_key` 与 `keyboard`；`input.rs` 的多行回车分支改为看 `enter_key != Newline`（替代 `submit_on_enter` 的判断，Shift+回车仍换行，修饰键规则不变）；`ime_text_state` 填三个新字段；`events.rs` 在 `on_key` 之后：`!key_handled`、普通回车、不在组合中时，`Next` 按 Tab 顺序移焦点，显式 `Done/Search/Go` 单行清除焦点（需要区分"显式"：`EditorState` 另存声明值）。
- [ ] 运行确认通过：`cargo test -p fenestra-core`。
- [ ] 提交：`feat(core): inputs declare a keyboard and an enter key`。

### Task 2: shell 的 EditorInfo 映射与推送

**Files:** `fenestra-shell/src/ime_sync.rs`（映射函数和测试，与平台无关）、`fenestra-shell/src/window.rs`（`sync_ime_text`）。

**Interfaces:**
- `pub(crate) struct EditorInfo { input_type: u32, action: i32, options: u32 }`
- `pub(crate) fn editor_info(state: &ImeTextState) -> EditorInfo`
- `ImeSync::held_editor_info(&self) -> Option<EditorInfo>`（替代 `held_masked`）。

- [ ] 写失败的测试（`ime_sync.rs` 内）：
  - `the_default_field_maps_as_before`：单行默认 → `(TEXT=1, Done=6, NO_FULLSCREEN|0x6)`，与现在 `sync_ime_text` 推送的值相同；密码 → 现在的密码值。
  - `each_keyboard_maps_to_its_class`：Number→2，Decimal→2|0x2000，Phone→3，Email→1|0x20，Url→1|0x10。
  - `actions_map_and_newline_keeps_enter`：Send→4、Search→3、Go→2、Next→5，均无 `NO_ENTER_ACTION`(0x40000000)；多行 Newline → 动作 1，带 `NO_ENTER_ACTION` 和 `MULTI_LINE`(0x20000)；多行 Send 带 `MULTI_LINE`、无 `NO_ENTER_ACTION`。
  - `a_masked_number_is_a_number_password`：Number+masked → 2|0x10(NUMBER_VARIATION_PASSWORD)，含 `NO_PERSONALIZED_LEARNING`。
  - `a_field_switch_repushes_editor_info`：`ImeSync` 推送 A 后，`held_editor_info` 等于 `editor_info(A)`；B 的信息不同。
- [ ] 运行确认失败：`cargo test -p fenestra-shell --lib ime_sync`。
- [ ] 实现映射函数；`ImeSync` 记录 held 的信息；`received()` 从 synced 复制三个新字段；`sync_ime_text` 在 `editor_info(now) != held` 时调用 `set_ime_editor_info(from_bits_retain(..), TextInputAction::from(action), from_bits_retain(..))`。
- [ ] 运行确认通过：`cargo test -p fenestra-shell`；Android `cargo check`。
- [ ] 提交：`feat(shell): the Android IME gets each field's keyboard and enter key`。

### Task 3: kit、文档、全量门控、推送

- [ ] kit `text_input`：`.keyboard()`、`.enter_key()`、`.on_submit(msg)`（回车时发出；测试 `on_submit_fires_on_enter`，用 Harness 在聚焦字段上按回车）。
- [ ] CHANGELOG：Added（键盘类型、回车键、下一项、显式完成/搜索/前往失焦、kit on_submit）；Breaking（`InputData`、`ImeTextState` 新增公开字段）。
- [ ] 全量门控；推送；触发 CI。

### Task 4: wechat-ui 与真机

- [ ] 聊天输入框 `.enter_key(EnterKey::Send)`；会话搜索框 `.enter_key(EnterKey::Search)`；`cargo test`（native）全部通过；本地提交。
- [ ] 一加 6（Gboard）：聊天输入框右下角是"发送"，按下发出消息；搜索框是搜索键，按下键盘收起；临时探针页（不提交）检查数字/电话/邮箱键盘；拼音输入中途不丢字。截图存笔记目录。清理 `debug.wechatui.*`，输入法仍是 Gboard。
