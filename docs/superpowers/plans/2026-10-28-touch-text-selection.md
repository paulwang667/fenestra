# 移动端文本选择、编辑菜单与剪贴板 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 触摸设备上的文字可以用手指选择、复制、剪切、粘贴（系统剪贴板），手柄、放大镜和编辑菜单按平台适配；wechat-ui 的消息气泡有"复制 / 选择文本 / 引用 / 删除"菜单。

**Architecture:**
- **剪贴板**：shell 新增移动端剪贴板模块，Android 走 JNI，iOS 走 UIPasteboard；这是工作区第二处允许 `unsafe` 的地方。
- **选择逻辑**：shell 告诉 core 当前输入来自触摸。core 新增 `touch_select` 模块，负责状态机、几何计算和编辑动作。手柄、菜单、放大镜作为框架自绘的界面元素，在 `Frame::paint_walk` 的浮层之后绘制。
- **测试驱动**：Harness 新增触摸动词，经过 shell 真实的 `Touch` 翻译器，所以测试里的手势和设备上一致。

**Tech Stack:** Rust；parley（选区与 hit test）；winit 0.30.13 分叉；jni 0.22.4（Android）；objc2 0.5.2 / objc2-ui-kit 0.2.2（iOS，与 winit 同版本）；vello 与 fenestra-raster。

**Spec:** `docs/superpowers/specs/2026-10-28-touch-text-selection-design.md`

## Global Constraints

- **构建环境**：`CARGO_TARGET_DIR=/private/tmp/claude-501/-Users-xxxx-projects-wechat-ui/06fb374e-ccba-4302-b42c-3e65fef847d8/scratchpad/target CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0`。
- **不要**运行 `cargo test --workspace`。
- **本地关卡**：`bash $S/fullcheck.sh <名字> <包…>`（`$S` 为上面的 scratchpad 目录）。只测改动涉及的包；raster goldens 不能新增失败（与 `before.txt` 对比）。
- **`unsafe`**：只允许在 `fenestra-raster/src/coretext.rs` 和本计划新增的 `fenestra-shell/src/mobile_clipboard/`（模块级 `#![allow(unsafe_code)]`）这两处。工作区保持 `unsafe_code = "deny"`。
- **只在触摸输入时启用**：鼠标、键盘、桌面、网页的行为一律不变，现有测试和截图对比照旧通过。
- **触摸目标**：手柄可触区域至少 44×44 逻辑像素。
- **菜单文字**：
  - 中文（`Fonts::locale()` 的语言是 `zh`）：剪切 / 复制 / 粘贴 / 全选；
  - 其它语言：Cut / Copy / Paste / Select All。
- **iOS 剪贴板**：只在执行"粘贴"时读取剪贴板；决定是否显示"粘贴"项只用 `hasStrings`（Android 用 `hasPrimaryClip`）。
- **推送与提交**：fenestra 推送到 fork main 已获授权（`git -c credential.helper= -c credential.helper='!gh auth git-credential' push -q https://github.com/paulwang667/fenestra.git main:main`，然后 `git update-ref refs/remotes/fork/main main`，再 `gh workflow run CI --repo paulwang667/fenestra --ref main`）。wechat-ui 只在本地提交，不推送，也不运行 `cargo fmt`。
- **设备测试**：一加 6（`ANDROID_SERIAL=d0305cb2`）。测完恢复 Gboard 为默认输入法，并清空 `debug.wechatui.*` 属性；不改系统设置。

## Review Focus

1. **滚动中的选区**：滚动容器里的文字被选中后，用户滚动列表，手柄应跟着文字移动，滚出视野就隐藏，滚回来再出现（任务 4 加测试 `handles_follow_scroll_and_hide_off_screen`）。
2. **组字中粘贴**：Android/iOS 输入法正在组字（拼音未上屏）时点"粘贴"，应先结束组字再替换，输入法收到的新状态里没有残留的组字区间（任务 5 加测试 `paste_while_composing_commits_first`）。
3. **选区目标在重建后变了**：选中消息后，这条消息被删除或文字变短，选区应被清掉或截断，不 panic（任务 3 加测试 `selection_survives_text_shrinking_and_removal`）。
4. **多行、中文和 emoji**：手柄拖过多行中文和 emoji 时，选区端点落在字形簇边界上，不会把 emoji 切开（任务 4 加测试 `handle_drag_snaps_to_grapheme_clusters`）。
5. **菜单贴边**：选区在屏幕顶部（安全区下方）时菜单翻到下方，屏幕很窄时水平方向夹在画布内（任务 5 加测试 `menu_flips_below_near_top_and_clamps_horizontally`）。

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `fenestra-core/src/clipboard.rs` | `Clipboard::has_text` |
| `fenestra-shell/src/mobile_clipboard/mod.rs`, `android.rs`, `ios.rs`（新） | 系统剪贴板（`unsafe` 例外） |
| `fenestra-shell/src/os_clipboard.rs`, `window.rs` | 按平台注入剪贴板，发出触摸来源 |
| `fenestra-core/src/events.rs` | `InputEvent::PointerSource`，长按、点按和拖动的触摸分支，`owns_touch_drag` |
| `fenestra-core/src/touch_select.rs`（新） | 触摸选择状态、手柄和菜单几何、编辑动作、绘制 |
| `fenestra-core/src/frame_state.rs` | 状态字段与公开查询 |
| `fenestra-core/src/frame.rs` | 构建时计算几何，`paint_walk` 末尾绘制，放大镜重画 |
| `fenestra-core/src/theme.rs` | `SelectionControls` |
| `fenestra-core/src/element.rs` | `Element::select_all_on` |
| `fenestra-shell/src/touch.rs` | 长按后继续拖动时转发移动 |
| `fenestra-shell/src/harness.rs` | 触摸动词、`haptics()`、`touch_selection()` |
| wechat-ui `native/src/ui.rs`，`native/tests/` | 消息菜单、引用、删除 |

---

### Task 1: 系统剪贴板（Android 与 iOS）

**Files:**
- Modify: `fenestra-core/src/clipboard.rs`
- Create: `fenestra-shell/src/mobile_clipboard/mod.rs`、`android.rs`、`ios.rs`
- Modify: `fenestra-shell/src/os_clipboard.rs`、`fenestra-shell/src/lib.rs`、`fenestra-shell/src/window.rs`（约 2019 行和 3149 行的剪贴板注入）
- Modify: 根 `Cargo.toml`（workspace 依赖和 `unsafe` 注释）、`fenestra-shell/Cargo.toml`、`CHANGELOG.md`
- Test: `fenestra-core/src/clipboard.rs` 单元测试；`fenestra-shell/tests/ios_pasteboard.rs`（`#![cfg(target_os = "ios")]`）

**Interfaces:**
- Produces:
  - `trait Clipboard { fn has_text(&mut self) -> bool { self.get().is_some() } }`（新增默认方法）；
  - `fenestra_shell::OsClipboard` 在 Android 和 iOS 上变为系统剪贴板。Android 上的构造是 `OsClipboard::android(app: AndroidApp)`；没有 `AndroidApp` 时 `OsClipboard::default()` 退回内存剪贴板。

- [ ] **Step 1: 写测试**
  - `MemoryClipboard`：`has_text` 初始为 `false`，`set("x")` 之后为 `true`。
  - `ios_pasteboard_round_trips`：`OsClipboard::default()` `set("fenestra-剪贴板")` 后，`has_text()` 为真，`get()` 等于该字符串。
- [ ] **Step 2: 运行，确认失败**
  - Run: `cargo test -p fenestra-core --lib clipboard`。Expected: 编译失败，没有 `has_text`。
- [ ] **Step 3: 在 `Clipboard` 上加 `has_text` 默认方法**，再跑一次 Step 2，Expected: PASS。
- [ ] **Step 4: 实现 `mobile_clipboard`**
  - 模块顶部写 `#![allow(unsafe_code)]`，并用文档注释说明它是第二处例外。
  - **Android**：
    - 从 `AndroidApp::vm_as_ptr()` 和 `activity_as_ptr()` 构造 jni 环境；
    - `getSystemService("clipboard")` 得到 `ClipboardManager`；
    - `get`：`getPrimaryClip()?.getItemAt(0).coerceToText(ctx)`；
    - `set`：`setPrimaryClip(ClipData.newPlainText("text", s))`；
    - `has_text`：`hasPrimaryClip()`；
    - JNI 异常时清除异常，返回 `None` 或 `false`。
  - **iOS**：`UIPasteboard::generalPasteboard()` 的 `string`、`setString` 和 `hasStrings`。
  - 依赖：
    - `jni = "0.22.4"` 放在 android target 依赖里；
    - `objc2 = "0.5.2"`、`objc2-foundation = "0.2.2"`（`NSString`）、`objc2-ui-kit = "0.2.2"`（`UIPasteboard`）放在 ios target 依赖里；
    - 版本与 Cargo.lock 里已有的一致，在根 `Cargo.toml` 注释里写明来源。
  - `os_clipboard.rs`：Android 和 iOS 的 `OsClipboard` 包住新实现。`window.rs` 在 Android 上有 `options.android_app` 时用 `OsClipboard::android(app.clone())`。
- [ ] **Step 5: 更新根 `Cargo.toml` 的 `unsafe` 注释**（例外为两处）**和 CHANGELOG**（Fixed：Android/iOS 用系统剪贴板）。
- [ ] **Step 6: 验证**
  - Android：`cargo clippy -p fenestra-shell --target aarch64-linux-android -- -D warnings`。
  - iOS：`cargo clippy -p fenestra-shell --all-targets --target aarch64-apple-ios-sim -- -D warnings`，Expected: 无警告。
  - iOS 模拟器：`cargo test -p fenestra-shell --test ios_pasteboard --target aarch64-apple-ios-sim --no-run`，然后 `xcrun simctl spawn "iPhone 17" <测试二进制>`，Expected: PASS。
- [ ] **Step 7: 关卡与提交**
  - `fullcheck.sh clip fenestra-core fenestra-shell` 全绿后提交：`feat(shell): system clipboard on Android and iOS`。

### Task 2: 触摸来源与 Harness 触摸动词

**Files:**
- Modify: `fenestra-core/src/events.rs`、`fenestra-core/src/frame_state.rs`、`fenestra-shell/src/window.rs`（`touch_input_main`，以及鼠标的 `CursorMoved` / `MouseInput`）、`fenestra-shell/src/harness.rs`、`fenestra-shell/src/touch.rs`
- Test: `fenestra-shell/tests/touch_harness.rs`

**Interfaces:**
- Produces:
  - `pub enum PointerSource { Mouse, Touch }`（core，`#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]`，默认 `Mouse`）；
  - `InputEvent::PointerSource(PointerSource)`；
  - `FrameState::pointer_source(&self) -> PointerSource`；
  - `Dispatch.selection_started: bool`（任务 3 置位）；
  - Harness 新增：
    - `long_press(&mut self, q: &Query)`；
    - `touch_tap(&mut self, q: &Query)`；
    - `touch_tap_at(&mut self, at: (f32, f32))`；
    - `touch_down_at(&mut self, at: (f32, f32))`；
    - `touch_move_to(&mut self, at: (f32, f32))`；
    - `touch_up(&mut self)`；
    - `haptics(&self) -> &[Haptic]`。
  - 触摸动词内部用 `crate::touch::Touch::new(true)`，时间沿用 Harness 的时钟。`long_press` 按下后调用 `pump(500.0)` 并执行 `tick`，然后抬起。触感反馈的判定与 shell 一致：`touch_haptic(long_press, consumed || selection_started)`。

- [ ] **Step 1: 写测试**
  - `a_long_press_opens_a_right_click_menu`：一个带 `on_right_click(Msg::Menu)` 的行，`h.long_press(&by::label("row"))` 后 `h.take_messages()` 含 `Msg::Menu`，`h.haptics() == [Haptic::LongPress]`。
  - `touch_input_reports_touch_and_mouse_resets_it`：`touch_tap` 后 `pointer_source() == Touch`；随后 `h.click(...)` 后为 `Mouse`。
  - `a_short_touch_hold_does_not_long_press`：按下、`pump(300.0)`、抬起，得到一次点击，没有右键。
- [ ] **Step 2: 运行，确认失败**：`cargo test -p fenestra-shell --test touch_harness`，Expected: 编译失败。
- [ ] **Step 3: 实现**
  - core 处理 `InputEvent::PointerSource`：只写入状态，不产生重绘。
  - shell：每批触摸事件前，如果来源变了，先发 `PointerSource(Touch)`；鼠标事件前，如果来源变了，先发 `PointerSource(Mouse)`。
  - Harness 的鼠标动词同样在来源变化时先发 `PointerSource(Mouse)`。
- [ ] **Step 4: 运行，确认通过**；`fenestra-shell` 现有测试全部照旧通过。
- [ ] **Step 5: 提交** `feat: tell core when input is touch; harness touch verbs`

### Task 3: 触摸选择——开始、清除和查询

**Files:**
- Create: `fenestra-core/src/touch_select.rs`
- Modify: `fenestra-core/src/events.rs`（`RightDown` 的触摸分支、点按、双击）、`frame_state.rs`、`frame.rs`（构建时计算几何）、`lib.rs`（导出）、`fenestra-shell/src/harness.rs`
- Test: `fenestra-shell/tests/touch_select.rs`

**Interfaces:**
- Produces（core 公开）：
  - `pub enum EditAction { Cut, Copy, Paste, SelectAll }`；
  - `pub struct TouchSelection`，字段：
    - `pub target: WidgetId`
    - `pub range: (usize, usize)`（字节，`start <= end`；只有光标时两者相等）
    - `pub start_handle: Option<kurbo::Rect>`、`pub end_handle: Option<kurbo::Rect>`（可触区域，至少 44×44；滚出视野时为 `None`）
    - `pub menu: Vec<(EditAction, kurbo::Rect)>`（菜单隐藏时为空）
    - `pub magnifier: Option<kurbo::Rect>`
  - `FrameState::touch_selection(&self) -> Option<&TouchSelection>`；Harness 上同名转发方法。
  - `owns_touch_drag` 新规则：
    - 输入框**不再**拥有触摸拖动；
    - 落点在当前手柄的可触区域内时拥有。

- [ ] **Step 1: 写测试**（全部用 Harness 触摸动词）
  - `long_press_selects_a_word_in_selectable_text`：文本 "hello wide world" `.selectable()`，长按 "wide" 的中心，`range == (6, 10)`，菜单为 `[Copy, SelectAll]`，`haptics == [LongPress]`。
  - `long_press_selects_a_word_in_an_input`：输入框内容 "foo bar"，长按 "bar"，`range == (4, 7)`。菜单为 `[Cut, Copy, Paste, SelectAll]`；`MemoryClipboard` 为空时没有 `Paste`。
  - `a_right_click_ancestor_wins_over_selection`：可选文本外面套一个 `on_right_click` 的行，长按后得到行的消息，`touch_selection()` 为 `None`。
  - `a_tap_elsewhere_clears_the_selection`。
  - `double_tap_in_an_input_selects_a_word`。
  - `a_tap_on_the_caret_opens_paste_and_select_all`：输入框已聚焦且有光标，剪贴板有文字，在光标处 `touch_tap_at`，得到 `range` 两端相等，菜单为 `[Paste, SelectAll]`。
  - `mouse_input_never_starts_a_touch_selection`：用鼠标右键、点击、双击做同样的操作，`touch_selection()` 一直是 `None`，桌面现有行为不变。
  - `selection_survives_text_shrinking_and_removal`（Review Focus 3）：选中后应用把文字改短，或删掉该元素并 `rebuild`。前者 `range` 被截断到新长度且落在字符边界上，后者 `touch_selection()` 为 `None`；都不 panic。
- [ ] **Step 2: 运行，确认失败**：`cargo test -p fenestra-shell --test touch_select`。
- [ ] **Step 3: 实现**
  - **长按**：`RightDown` 时，如果 `pointer_source == Touch` 且命中链上没有 `on_right_click` / `on_right_click_at`：
    - 只读文本复用 `fonts.static_select(..., count = 2, ...)` 选词，写入 `static_sel`；
    - 输入框聚焦后用 `input::select_word_at`；
    - 置 `Dispatch.selection_started`，建立 `TouchSelection`。
  - **点按**：触摸下的单击和双击，按设计文档"手势"一节处理。点光标本身弹出菜单，判定为与光标的水平距离不超过 22 逻辑像素（**我的决定**）。
  - **菜单项规则**：
    - 只读文本：`Copy`、`SelectAll`；
    - 输入框：`Cut`、`Copy`（选区非空且非 masked 时）、`Paste`（`clipboard.has_text()` 时）、`SelectAll`。
  - **几何**：在 `build_frame` 末尾计算。手柄位置取选区首尾字形的位置（parley 的选区矩形）；元素在其滚动容器的可见区域之外时手柄为 `None`。
- [ ] **Step 4: 运行，确认通过**，并跑 `fenestra-core` 和 `fenestra-shell` 现有测试。
- [ ] **Step 5: 提交** `feat(core): touch text selection (long press, taps, query)`

### Task 4: 拖动手柄、长按后拖动、边缘自动滚动

**Files:**
- Modify: `fenestra-core/src/touch_select.rs`、`events.rs`、`fenestra-shell/src/touch.rs`（`Mode::LongPressed` 时移动转发为 `PointerMove`，抬起时发 `PointerUp`）
- Test: `fenestra-shell/tests/touch_select.rs`

**Interfaces:**
- Consumes: Task 3 的 `TouchSelection`、`owns_touch_drag`。

- [ ] **Step 1: 写测试**
  - `dragging_the_end_handle_extends_the_selection`：选中 "wide" 后，把 `end_handle` 中心拖到 "world" 末尾，`range == (6, 16)`。拖动过程中 `menu` 为空，`magnifier` 为 `Some`；抬起后菜单恢复，`magnifier` 为 `None`。
  - `handles_cross_and_swap_ends`：把起点手柄拖过终点，`range` 仍满足 `start <= end`。
  - `a_handle_drag_does_not_scroll`：同一段文字在滚动容器里，拖手柄后滚动偏移不变。
  - `a_drag_on_text_without_a_selection_scrolls`：滚动容器里的输入框，没有选区时拖动，滚动偏移改变，没有产生选区。
  - `long_press_then_drag_moves_the_caret`：在输入框里长按后不松手向右移动，光标（`range` 的两端）跟着移动，`magnifier` 为 `Some`。
  - `dragging_into_the_edge_band_autoscrolls`：在可滚动的长文本里，把手柄拖到视口底边 24 逻辑像素以内并停住，`pump(300.0)` 后滚动偏移增加，选区终点跟着变化。
  - `handles_follow_scroll_and_hide_off_screen`（Review Focus 1）：选中后用 `wheel` 或触摸滚动，`end_handle` 的 y 坐标随之变化；滚到看不见时为 `None`，滚回来恢复。
  - `handle_drag_snaps_to_grapheme_clusters`（Review Focus 4）：文本 "中文👍🏽结尾" 跨两行，手柄拖到 emoji 中间的坐标，`range.1` 落在字形簇边界上（`text.is_char_boundary`，且不在 👍 和 🏽 之间）。
- [ ] **Step 2: 运行，确认失败。**
- [ ] **Step 3: 实现**
  - 触摸下在手柄可触区域内按下，开始拖动手柄。
  - `PointerMove` 时用 parley hit test 更新对应端点（复用 `static_extend` 和 `input::pointer_drag` 的选区逻辑）。
  - 边缘带宽 24 逻辑像素。按住期间每帧滚动 `(24 − 距离) × 0.5` 逻辑像素，并请求下一帧（与惯性滑动保持动画的机制相同）。
  - 放大镜矩形以手指为中心、位于手指上方 40 逻辑像素处：Android 尺寸 100×48，iOS 胶囊 110×44（**我的决定**，参照 Flutter）。
- [ ] **Step 4: 运行，确认通过**，并确认 `fenestra-shell` 现有触摸测试照旧通过。
- [ ] **Step 5: 提交** `feat(core): drag selection handles, long-press caret drag, edge autoscroll`

### Task 5: 编辑菜单的动作与位置

**Files:**
- Modify: `fenestra-core/src/touch_select.rs`、`events.rs`、`input.rs`（把剪切、复制、粘贴、全选从 `handle_key_inner` 里提出来，供菜单复用）
- Test: `fenestra-shell/tests/touch_select.rs`

**Interfaces:**
- Produces: `pub(crate) fn apply_edit(action: EditAction, ...)`，由键盘快捷键和菜单共用。快捷键的行为不变。

- [ ] **Step 1: 写测试**
  - `copy_from_the_menu_puts_the_selection_on_the_clipboard`：点 `Copy` 的矩形后，剪贴板为 "wide"，选区和菜单消失。
  - `cut_and_paste_in_an_input`：选中 "bar" 后点 `Cut`，内容变为 "foo "，剪贴板为 "bar"；再在光标处弹出菜单并点 `Paste`，内容回到 "foo bar"。之后 `FrameState::ime_text_state()` 的文本和选区与编辑器一致。
  - `select_all_from_the_menu`：只读文本和输入框都能全选，菜单保持显示。
  - `a_masked_input_offers_no_cut_or_copy`。
  - `paste_while_composing_commits_first`（Review Focus 2）：先送 `InputEvent::ImeState`，组字区间为 "ni"；点 `Paste` 后 `ime_text_state().compose` 为 `None`，文本里没有残留的拼音。
  - `menu_flips_below_near_top_and_clamps_horizontally`（Review Focus 5）：设 `safe_area.y0 = 47`，选中第一行的词，菜单矩形的 `y0` 大于选区底部；画布宽 200 时，菜单的 `x0 >= 0` 且 `x1 <= 200`。
  - `menu_labels_follow_the_locale`：`set_locale(Some("zh-CN"))` 时，可访问树里菜单按钮的 label 为 "复制"；`None` 时为 "Copy"。
- [ ] **Step 2: 运行，确认失败。**
- [ ] **Step 3: 实现**
  - 触摸点按落在菜单项矩形内时执行对应动作，不再传给下面的元素。
  - 菜单按钮进入可访问树（role 为 button，label 按语言）。
  - **位置**：选区上方 8 逻辑像素；放不下（进入 `safe_area` 顶部）时放到选区下方，避开手柄；水平方向居中于选区，并夹在画布内（左右各留 8 逻辑像素）。
  - **尺寸**：菜单项高度 44 逻辑像素，宽度按文字测量，左右内边距各 12。
- [ ] **Step 4: 运行，确认通过**，并跑 `fenestra-core` 的输入框测试，确认快捷键行为不变。
- [ ] **Step 5: 提交** `feat(core): edit menu actions for touch selection`

### Task 6: 绘制——手柄、菜单、放大镜，按平台适配

**Files:**
- Modify: `fenestra-core/src/theme.rs`、`touch_select.rs`（`paint`）、`frame.rs`（`paint_walk` 在浮层和退出动画之后调用；放大镜重画）
- Test: `fenestra-shell/tests/touch_select_golden.rs`，截图放在 `fenestra-shell/tests/snapshots/`

**Interfaces:**
- Produces:
  - `pub enum SelectionControls { Material, Cupertino }`；
  - `Theme.selection_controls: SelectionControls`，默认值：`target_os = "ios"` 为 `Cupertino`，其余为 `Material`。所有 `Theme` 构造函数都要设置这个字段。

- [ ] **Step 1: 写截图对比测试**
  - 用 `assert_png_snapshot`，同一场景（输入框里选中一个词，菜单显示）各截一张：`touch_select_material`、`touch_select_cupertino`。
  - 再截两张拖动中的场景（显示放大镜）：`touch_magnifier_material`、`touch_magnifier_cupertino`。
  - 再用 `FENESTRA_RENDERER=raster` 运行，与 vello 的结果相比，在现有文字预算（channel 24、3%）内一致。
- [ ] **Step 2: 运行，确认失败**（没有截图，或画面里没有界面元素）。
- [ ] **Step 3: 实现**，样式参照 Flutter 的 Material 和 Cupertino 选择控件：
  - **Material**：
    - 手柄：水滴形，22 逻辑像素，颜色 `theme.accent`，挂在选区端点下方；
    - 菜单：圆角 8 的卡片，填充 `theme.surface_raised`，阴影 `theme.shadow(ShadowToken::Md)`，文字颜色 `theme.text`；
    - 放大镜：圆角 8 的矩形，加 `ShadowToken::Sm`。
  - **Cupertino**：
    - 手柄：2 逻辑像素宽的竖线加直径 8 的圆点，颜色 `theme.accent`；起点的圆点在上方，终点的在下方；
    - 菜单：圆角 8 的横条，填充 `#1C1C1E` 不透明度 0.92，文字白色，按钮之间是 0.5 逻辑像素的分隔线；
    - 放大镜：胶囊形。
  - **放大镜内容**：把整棵树在 `Affine::scale(1.25)`（以手指位置为中心）和放大镜形状的裁剪下再画一遍，不读回像素。
- [ ] **Step 4: 生成截图并人工查看**，确认样式符合上面的描述，再提交截图。之后在两个后端各跑一遍，Expected: PASS。
- [ ] **Step 5: 提交** `feat(core): draw selection handles, edit menu and magnifier per platform`

### Task 7: 程序化进入选择

**Files:**
- Modify: `fenestra-core/src/element.rs`、`frame.rs` 或 `events.rs`（处理请求，参照 `focus_request` 的实现）
- Test: `fenestra-shell/tests/touch_select.rs`

**Interfaces:**
- Produces: `Element::select_all_on(self, request: u64) -> Self`，语义与 `focus_on` 相同：元素带上一个上次没带过的 token 时，全选它的文字，显示手柄和菜单，并把来源视为触摸。作用于 `.selectable()` 的文本，或输入框。
- **Ruling**：设计文档要求"按 id 或 label 找元素"。fenestra 没有命令式的 `Cmd`，`focus_on` 和 `scroll_to` 都是挂在元素上的声明式 token，这里沿用这个惯例。

- [ ] **Step 1: 写测试**
  - `select_all_on_selects_and_shows_the_menu`：应用状态里的 token 从 0 变为 1 并 `rebuild`，`range == (0, len)`，菜单为 `[Copy, SelectAll]`。
  - `the_same_token_does_not_reselect`：用户点别处取消选择后再 `rebuild`（token 未变），`touch_selection()` 仍为 `None`。
  - `select_all_on_a_non_selectable_text_does_nothing`：没有 panic，没有选区。
- [ ] **Step 2: 运行，确认失败。** **Step 3: 实现。** **Step 4: 运行，确认通过。**
- [ ] **Step 5: 关卡、CHANGELOG（Added：触摸文本选择、编辑菜单、放大镜、`select_all_on`、`SelectionControls`）、提交，并推送到 fork main 触发 CI。**
  - Run: `fullcheck.sh touchsel fenestra-core fenestra-shell fenestra-kit`。Expected: 全绿，raster goldens 没有新增失败。

### Task 8: wechat-ui 消息菜单（复制、选择文本、引用、删除）

**Files:**
- Modify: wechat-ui `native/src/ui.rs`（`Msg`、`Model`、`message_bubble` / `text_bubble` / `code_card`、输入区、发送）
- Test: `native/tests/message_menu.rs`（新）
- Modify: `native/Cargo.lock`、`native/android/Cargo.lock`（`cargo update -p fenestra`，拉到 Task 7 推送的提交）

**Interfaces:**
- Consumes: `Element::select_all_on`、`Harness::long_press` / `right_click` / `touch_selection`。
- Produces（wechat-ui 内部）：
  - `Msg::OpenMessageMenu(MsgId)`、`Msg::CloseMessageMenu`、`Msg::CopyMessage(MsgId)`、`Msg::SelectMessageText(MsgId)`、`Msg::QuoteMessage(MsgId)`、`Msg::CancelQuote`、`Msg::DeleteMessage(MsgId)`，其中 `MsgId` 用现有消息的 id 类型；
  - `Model.message_menu: Option<MsgId>`、`Model.quoting: Option<MsgId>`、`Model.select_request: (MsgId, u64)`；
  - 消息结构增加 `quote: Option<Quote>`，`Quote { sender: String, excerpt: String }`，摘要最多 40 个字符，超出用"…"截断。

- [ ] **Step 1: 写测试**（Harness，仿照 `tests/mobile.rs` 的写法）
  - `right_click_on_a_bubble_opens_the_message_menu`：菜单里依次是 "复制"、"选择文本"、"引用"、"删除"（按 label 查询）。
  - `long_press_on_a_bubble_opens_the_menu_not_a_selection`：`touch_selection()` 为 `None`，菜单打开。
  - `copy_message_copies_the_whole_text`：代码卡片复制的是代码本身。
  - `select_text_selects_the_whole_message`：`touch_selection().range == (0, 正文长度)`。
  - `quote_shows_a_bar_and_sending_attaches_it`：输入区上方出现"引用"条；点 × 后消失；发送后新消息的 `quote` 为 `Some`，气泡里显示摘要。
  - `delete_removes_the_message`：没有确认框。
- [ ] **Step 2: 运行，确认失败**：`cargo test --test message_menu`（在 `native/` 下）。
- [ ] **Step 3: 实现**：仿照会话列表行的 `on_right_click` 加 `context_menu`；引用条和气泡里的引用样式参照现有的气泡样式。
- [ ] **Step 4: 运行 wechat-ui 现有测试，确认全部通过。**
- [ ] **Step 5: 本地提交**（不推送）：`feat(native): message menu (copy, select text, quote, delete)`

### Task 9: 设备验证

- [ ] **Step 1: 一加 6**
  - 用 `native/android/build.sh` 安装（带 `ANDROID_SERIAL=d0305cb2`）。
  - 按设计文档"目标"一节逐条测试：长按选词、拖手柄、放大镜、菜单四个动作、Gboard 组字时粘贴。
  - 剪贴板双向：在别的应用里复制，到 wechat-ui 里粘贴；在 wechat-ui 里复制，到别的应用里粘贴。
  - 测完恢复 Gboard 为默认输入法，清空 `debug.wechatui.*`。
- [ ] **Step 2: iOS 模拟器**：用不带本地补丁的构建，做同样的测试；剪贴板从 Mac 侧确认（模拟器与 Mac 共享剪贴板）。
- [ ] **Step 3: 记录**：结果和截图写进 notes 目录的 `IOS-SPIKE.md`。发现问题时回到对应任务修复（先写测试）。
