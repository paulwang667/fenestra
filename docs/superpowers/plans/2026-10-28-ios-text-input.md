# iOS 第二阶段（键盘、中文输入、触控板、触感）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 在 iOS 上：键盘不再遮住输入框，拼音等输入法能组字上屏，键盘类型和回车键文字跟着文本框走，触控板能滚动，长按有触感反馈。

**Architecture:** winit fork 在 iOS 视图上实现 UITextInput，作用在一份文本镜像上（纯 Rust 的 `TextMirror`，可在主机上测试），每次编辑发出 `Ime::TextState`；UITextInputTraits 和键盘区域、间接滚动也在 fork 里。fenestra 在 `ios-text-input` feature 下把 Android 的同步逻辑扩展到 iOS。wechat-ui 改为指向 fork，并实现触感钩子。

**Spec:** `docs/superpowers/specs/2026-10-28-ios-text-input-design.md`

## Global Constraints

- winit fork：本地 `~/projects/winit-fork`，分支 `fenestra/v0.30.13-ios-text-input`（基于 `origin/fenestra/v0.30.13-android-ime`），objc2 0.5 / objc2-ui-kit 0.2.2。用户已授权推送这个 fork。
- fenestra：`unsafe_code = "forbid"`；用到 fork 新接口的代码放在 `cfg(all(target_os = "ios", feature = "ios-text-input"))` 下；不开 feature 时 iOS 行为不变。
- 镜像里存字节偏移，UIKit 用 UTF-16 偏移，换算只在 fork 里做。
- 本地门控只跑受影响的包（`fullcheck.sh <name> <pkgs>`）；共享 `CARGO_TARGET_DIR`，`CARGO_INCREMENTAL=0`。
- wechat-ui 只本地提交，不跑 `cargo fmt`。

## Review Focus

1. 应用推送状态与输入法编辑交错（组字中途应用改了文本、焦点切到另一个框）：镜像以应用推送为准，组字被取消，不出现旧文本回写。→ Task 1 测试 `an_app_push_cancels_composition`；fenestra 沿用 `ImeSync` 的过期回声过滤。
2. 含中文和 emoji 的文本里的 UTF-16 与字节偏移换算，以及落在代理对中间的偏移：钳到字符边界，不 panic。→ Task 1 测试 `offsets_inside_a_surrogate_pair_snap_to_the_char`。
3. 键盘收起时（`WillHide`），以及键盘高度变化时（切换输入法使候选栏增高）：安全区随之恢复或更新。→ Task 3 测试 `keyboard_overlap_*`。
4. 没调用 `set_text_input_state` 的应用：`insertText`、`deleteBackward` 仍发 KeyboardInput，行为和第一阶段一样。→ Task 2 写进代码路径，Task 7 用不开 feature 的构建在模拟器上确认。
5. 回车键：没有组字时变成 Enter，组字中途按回车则确认组字。→ Task 1 测试 `newline_without_marked_text_is_enter`、`newline_while_composing_commits`。

---

### Task 1: 纯 Rust 的文本镜像（fork）

**Files:** Create `src/platform_impl/ios_text_mirror.rs`；Modify `src/platform_impl/mod.rs`（`#[cfg(any(ios_platform, test))] pub(crate) mod ios_text_mirror;`）

**Interfaces:**
- Produces:
  - `pub(crate) struct TextMirror { pub text: String, pub selection: (usize, usize), pub marked: Option<(usize, usize)> }`（字节偏移）。
  - 偏移换算：`fn utf16_len(&self) -> usize`、`fn byte_at(&self, utf16: usize) -> usize`（落在字符中间时钳到所在字符的起点）、`fn utf16_at(&self, byte: usize) -> usize`。
  - 编辑操作（参数都是 UTF-16 偏移，都返回 `bool` 表示内容或选区是否变化）：
    - `fn replace(&mut self, range: (usize, usize), text: &str) -> bool`：替换区间，光标落到插入文本之后，清除组字。
    - `fn insert(&mut self, text: &str) -> bool`：有组字时替换组字区间，否则替换选区。
    - `fn set_marked(&mut self, text: &str, selected: (usize, usize)) -> bool`：替换组字区间（没有组字时替换选区）；新的组字区间覆盖插入的文本，`selected` 相对它。
    - `fn unmark(&mut self) -> bool`、`fn delete_backward(&mut self) -> bool`（删除选区，光标时删前一个字符，按字素簇用 `char` 边界）、`fn set_selection(&mut self, range: (usize, usize)) -> bool`。
  - `pub(crate) enum InsertAction { Edit, Enter }` 和 `fn classify_insert(&self, text: &str) -> InsertAction`：`"\n"` 且没有组字时返回 `Enter`。

- [ ] **Step 1:** 写测试：`replace_counts_utf16_not_bytes`（"你好" 的 UTF-16 区间 (1,2) 替换为 "们"，得到 "你们"）、`set_marked_then_insert_commits`、`set_marked_then_unmark_keeps_text`、`delete_backward_removes_one_char_or_the_selection`（含 emoji "a😀"）、`offsets_inside_a_surrogate_pair_snap_to_the_char`、`newline_without_marked_text_is_enter`、`newline_while_composing_commits`（组字中 `classify_insert("\n")` 返回 `Edit`，`insert` 只确认组字，不插入换行）、`an_app_push_cancels_composition`（应用推送新的 `TextMirror` 时 `marked` 取应用推送的值）。
- [ ] **Step 2:** `cargo test --lib ios_text_mirror` → 编译失败（RED）。
- [ ] **Step 3:** 实现。
- [ ] **Step 4:** 测试通过；`cargo clippy --lib -- -D warnings` 通过（主机）。
- [ ] **Step 5:** 提交 `ios: a byte-offset text mirror for UITextInput`。

### Task 2: UITextInput、键盘特征、公开接口（fork）

**Files:** Create `src/platform_impl/ios/text_input.rs`（WinitTextPosition、WinitTextRange 类，以及协议方法的辅助函数）；Modify `src/platform_impl/ios/view.rs`、`window.rs`、`src/platform/ios.rs`

**Interfaces:**
- Produces（`platform::ios`）：
  - `pub struct TextInputState { pub text: String, pub selection: (usize, usize), pub compose: Option<(usize, usize)> }`
  - `pub struct TextInputTraits { pub keyboard: KeyboardType, pub return_key: ReturnKeyType, pub secure: bool, pub autocorrect: bool, pub autocapitalize: bool }`，实现 `Default`。
  - `#[non_exhaustive] pub enum KeyboardType { Default, NumberPad, DecimalPad, PhonePad, EmailAddress, Url }`
  - `#[non_exhaustive] pub enum ReturnKeyType { Default, Send, Search, Go, Done, Next }`
  - `WindowExtIOS::set_text_input_state(&self, Option<TextInputState>)`、`WindowExtIOS::set_text_input_traits(&self, TextInputTraits)`。
- 视图：
  - ivars 新增 `mirror: RefCell<Option<TextMirror>>`、`traits: Cell<TextInputTraits>`、`input_delegate`（弱引用）、`cursor_area: Cell<CGRect>`。
  - 实现 UITextInput 的全部必需方法；UITextRange 的 `start`/`end`/`isEmpty` 由子类实现。
  - `tokenizer` 返回 `UITextInputStringTokenizer::initWithTextInput(self)`，缓存在 ivar 里。
  - 输入法编辑先改镜像；有变化就发 `Ime::TextState`（字节偏移），前后调用 `inputDelegate` 的 `textWillChange`/`textDidChange`。应用推送新状态时也调用这两个方法。
  - `mirror` 为 `None` 时，`insertText`、`deleteBackward` 走原来的 KeyboardInput 路径。
  - `insertText` 遇到 `classify_insert` 返回 `Enter` 时，发 Enter 的按下和抬起，不改镜像。
  - 特征属性 `keyboardType`、`returnKeyType`、`isSecureTextEntry`、`autocorrectionType`、`autocapitalizationType` 读 ivar；修改特征时，若视图是第一响应者就调用 `reloadInputViews`。
  - `set_ime_cursor_area` 在 iOS 上记录到 `cursor_area`，供 `caretRectForPosition`/`firstRectForRange` 返回。
- [ ] **Step 1:** 实现上面这些。UIKit 部分无法在主机上测试，逻辑尽量放进 Task 1 的纯函数。
- [ ] **Step 2:** `cargo build --target aarch64-apple-ios-sim` 和 `cargo clippy --target aarch64-apple-ios-sim -- -D warnings` 通过；主机测试仍通过。
- [ ] **Step 3:** 提交 `ios: UITextInput over the text mirror, text input traits`。

### Task 3: 键盘区域与间接滚动（fork）

**Files:** Modify `src/platform_impl/ios/view.rs`、`window.rs`；纯逻辑放进 `ios_text_mirror.rs` 旁边的 `src/platform_impl/ios_keyboard.rs`（cfg 同 Task 1）。

**Interfaces:**
- `pub(crate) fn keyboard_overlap(window: CGRectLike, keyboard_end: CGRectLike) -> f64`（窗口坐标里键盘与窗口底部的重叠高度，键盘在屏幕外或宽度为 0 时为 0）。纯函数，用 `(x, y, w, h)` 元组表示矩形。
- 视图在创建时注册 `UIKeyboardWillChangeFrameNotification` 和 `UIKeyboardWillHideNotification`（复用 `notification_center::create_observer`），把重叠高度写进视图 ivar `keyboard_overlap: Cell<f64>`，然后发 `WindowEvent::Resized(当前整屏尺寸)`。
- `safe_area_screen_space`：底边取 `max(safeAreaInsets.bottom, keyboard_overlap)`。
- 间接滚动：视图始终注册一个 UIPanGestureRecognizer，`allowedScrollTypesMask = UIScrollTypeMaskAll`，`allowedTouchTypes = []`；回调里发 `WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(增量, 物理像素), phase }`。
- [ ] **Step 1:** 写测试 `keyboard_overlap_counts_only_the_part_over_the_window`、`keyboard_overlap_is_zero_when_hidden_or_offscreen` → RED → 实现 → GREEN。
- [ ] **Step 2:** 接入视图和窗口。用 iOS 目标做 build 和 clippy。
- [ ] **Step 3:** 提交 `ios: report the keyboard in the safe area; indirect scrolling as mouse wheel`。

### Task 4: fenestra：文本同步、特征、光标区域

**Files:** Modify `fenestra-shell/Cargo.toml`（`ios-text-input = []`）、`fenestra/Cargo.toml`（`ios-text-input = ["fenestra-shell/ios-text-input"]`）、`fenestra-shell/src/ime_sync.rs`、`fenestra-shell/src/window.rs`

**Interfaces:**
- `ime_sync::Traits { keyboard: IosKeyboard, return_key: IosReturn, secure: bool, autocorrect: bool, autocapitalize: bool }`，以及 `pub(crate) fn text_input_traits(field: &ImeTextState) -> Traits`。用 fenestra 自己的类型，在主机上可测；只在开了 feature 的 iOS 代码里转换成 winit 的类型。
- 映射：
  - Number→NumberPad，Decimal→DecimalPad，Phone→PhonePad，Email→EmailAddress，Url→Url，其它→Default；
  - EnterKey 中 Send、Search、Go、Done、Next 对应同名，Newline 和 Default 对应 Default；
  - `masked` 时 `secure`；密码、邮箱、网址、数字类关闭自动更正和自动大写，其余开启。
- `sync_ime_text`：在 `cfg(any(android, all(ios, feature = "ios-text-input")))` 下编译。iOS 分支推送 `TextInputState`（字节偏移，不换算）；特征变了就推送 `set_text_input_traits`；没有聚焦文本框时推送 `None`。`Ime::TextState` 的接收分支对这个 cfg 也生效。
- 光标区域：每帧把聚焦文本框的光标矩形（逻辑坐标）用 `set_ime_cursor_area` 发出，只在矩形变化时发。
- [ ] **Step 1:** 写测试 `ios_traits_map_each_keyboard`、`ios_traits_map_each_enter_key`、`a_password_turns_off_suggestions`（与 `editor_info` 的测试一一对应）→ RED → 实现 → GREEN。
- [ ] **Step 2:** 用本地 path 补丁指向 fork，`cargo clippy -p fenestra-shell --target aarch64-apple-ios-sim --features ios-text-input -- -D warnings` 通过；不开 feature 时 iOS 和主机的 clippy 也通过。
- [ ] **Step 3:** 提交 `feat(shell): iOS text input through the winit fork (ios-text-input)`。

### Task 5: fenestra：安全区变化时把焦点滚进视野

**Files:** Modify `fenestra-core/src/frame.rs`（焦点滚进视野的触发条件）

- 现在的触发条件 `resized_under` 比较的是整窗尺寸 `revealed_size != size`。改为比较应用拿到的布局区域，即整窗减去 `state.safe_area`。键盘升起只改变安全区，也要能触发。
- [ ] **Step 1:** 写测试 `a_growing_safe_area_reveals_the_focused_field`：滚动视图底部有一个已聚焦的文本框，`safe_area.y1` 从 0 变为 300 后，文本框完整落在安全区上方 → RED → 实现 → GREEN。
- [ ] **Step 2:** `cargo test -p fenestra-core` 通过。
- [ ] **Step 3:** 提交 `fix(core): a safe-area change scrolls the focused field into view`。

### Task 6: fenestra：iOS 触感钩子

**Files:** Modify `fenestra-shell/src/window.rs`（`WindowOptions::with_ios_haptic(fn(Haptic))`、调用点与 Android 并列）

- 长按产生应用消息时（复用 `touch_haptic` 的判定）调用钩子。不依赖 fork，不放在 feature 下，只在 iOS 编译。
- [ ] **Step 1:** 把判定后调用钩子的逻辑抽成一个可在主机上测试的函数，或者复用 `touch_haptic` 的现有测试；写测试 `an_ios_haptic_hook_is_called_for_a_consumed_long_press`。
- [ ] **Step 2:** iOS 和主机 clippy 通过；提交 `feat(shell): with_ios_haptic`。

### Task 7: 门控、推送、wechat-ui、验证

- [ ] fenestra：CHANGELOG（Added），运行 `fullcheck.sh ios2 fenestra-core fenestra-shell fenestra`，额外跑一次开了 feature 的 iOS clippy（用 fork 补丁）。推送 fenestra。
- [ ] fork：用 iOS 目标做 build/clippy，主机测试和 `cargo fmt` 通过；推送分支 `fenestra/v0.30.13-ios-text-input`。
- [ ] wechat-ui：
  - `native/Cargo.toml` 和 `native/android/Cargo.toml` 把 winit 指向 fork 的新提交；
  - iOS 构建开启 `fenestra/ios-text-input`（target 专属依赖）；
  - 新增 iOS 触感模块（objc2 的 `UIImpactFeedbackGenerator`），`main.rs` 在 iOS 上调用 `with_ios_haptic`；
  - 本地提交，`cargo test` 通过。
- [ ] 模拟器验证（不带补丁），覆盖规格里的 1–5 项，截图存进笔记目录。
- [ ] Android：一加 6 连上后回归验证；没连上就记录未验证。

### Task 8: 审查

- [ ] 由独立的 opus 审阅者审查 fork、fenestra 和 wechat-ui 的全部改动，按 Review Focus 逐条检查。Critical 和 Important 级别的问题修一轮，每项先写失败的测试；Minor 记进账本。
