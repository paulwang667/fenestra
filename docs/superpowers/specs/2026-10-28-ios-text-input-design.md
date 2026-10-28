# iOS 第二阶段：键盘、中文输入与触控 设计

## 为什么做

第一阶段（`2026-10-28-ios-bring-up-design.md`）之后，wechat-ui 在 iOS 模拟器上已经可用，但聊天这个核心场景还用不起来。用户在 2026-10-28 让我开始第二阶段，流程由我自行推进。卡住的问题都在 winit 0.30 的 iOS 视图上：

- 弹出键盘不缩小窗口，winit 也不报告键盘位置，所以**键盘盖住聊天输入框**，打字时看不到自己输入的内容。
- 视图只实现了 UIKeyInput，没有实现 UITextInput，所以**拼音等输入法的组字（marked text）进不来**，中文打不出。
- UITextInputTraits 是空的，**键盘类型和回车键文字**（数字键盘、"发送"、"搜索"）没有设置。Android 上这些已经做了。
- 没有接收间接滚动（触控板、鼠标滚轮）的手势识别器，所以**触控板滚动无效**。
- **长按没有触感反馈**。Android 上已经有了。

## 目标

1. 文本框获得焦点、键盘弹出时，它自动移到键盘上方；键盘收起后布局恢复。
2. 系统拼音键盘能打中文：组字过程中，拼音显示在文本框里（带组字标记）；选候选词后上屏。退格、光标移动、系统自动更正也都正确。
3. 键盘类型和回车键文字跟着文本框走，映射和 Android 的 `editor_info` 一致：数字、小数、电话、邮箱、网址、密码（关闭联想和自动更正），回车键为发送、搜索、前往、完成、下一项、换行。
4. 触控板双指滑动和鼠标滚轮能滚动列表，和桌面的滚轮行为一致。
5. 长按触发菜单时有一次触感反馈（UIImpactFeedbackGenerator），和 Android 的 `with_android_haptic` 对称。

## 不做什么

- 键盘弹出或收起的动画跟随：布局直接跳到最终位置，和 Android 的 adjustResize 一样。
- 文本框里的放大镜、选择手柄、系统编辑菜单（拷贝、粘贴弹出条）。
- 听写（语音输入）和手写输入的专门处理。它们走 UITextInput，应该能用，但不专门验证。
- 真机、iPad 的分屏和浮动键盘、横屏。
- 把这些改动提交给 winit 上游。

## 怎么做

### winit fork（`paulwang667/winit`，基于 0.30.13，已有 Android 的 IME 提交）

在 iOS 视图上加上下面几部分，全部在 `platform_impl/ios`，对外接口放在 `platform::ios` 的扩展 trait 里。

1. **文本状态镜像（UITextInput）**
   - 视图保存一份文本状态：文本、选区、组字区间，都是字节偏移。
   - 应用通过 `WindowExtIOS::set_text_input_state(Option<TextInputState>)` 写入。语义和 Android 的 `AndroidApp::set_text_input_state` 相同：焦点切换、点击移动光标、应用改了内容时调用。`None` 表示没有文本框获得焦点。
   - 视图实现 UITextInput 协议，所有查询和修改都作用在这份镜像上：
     - 文本与位置：`textInRange`、`replaceRange:withText:`、`selectedTextRange`、`markedTextRange`、`setMarkedText:selectedRange:`、`unmarkText`、`beginningOfDocument`/`endOfDocument`；
     - 位置运算：`positionFromPosition:offset:`、`offsetFromPosition:toPosition:`、`comparePosition:toPosition:`、`textRangeFromPosition:toPosition:`；
     - 几何：`firstRectForRange:`、`caretRectForPosition:`、`closestPositionToPoint:` 等。返回应用最后一次通过 `set_ime_cursor_area` 给出的光标区域，iOS 上不再忽略这个调用。
     - 还要提供 UITextPosition/UITextRange 子类、`inputDelegate` 和 `tokenizer`。
   - UIKit 那边用 UTF-16 偏移，镜像里存字节偏移，换算在 fork 里完成。
   - 输入法每改一次，都发出 `Ime::TextState { text, selection, compose }`，复用 Android 的事件。
   - 应用写入新状态时，fork 通知 `inputDelegate`（`textWillChange`/`textDidChange`、`selectionWillChange`/`selectionDidChange`），让输入法丢掉过期的上下文。
   - **回车键**：没有组字时 `insertText:@"\n"` 不改镜像，发一次 Enter 的 KeyboardInput 按下和抬起，和 Android 把 GameActivity 的编辑动作变成 Enter 一样。fenestra 照常处理 Enter：单行框执行回车动作（发送、搜索），多行框插入换行。（我的决定，审阅后补充）
   - 其它的 `insertText`、`deleteBackward` 改为修改镜像并发 `Ime::TextState`，不再发 KeyboardInput。没有写入状态时（应用没调用 `set_text_input_state`）保持现在的 KeyboardInput 行为，不用这个接口的应用不受影响。
2. **键盘特征（UITextInputTraits）**
   - 新增 `WindowExtIOS::set_text_input_traits(TextInputTraits)`，字段包括：`keyboard_type`、`return_key_type`、`secure`、`autocorrect`、`autocapitalize`、`enables_return_key_automatically`。
   - 视图实现对应的属性。视图已是第一响应者时，修改特征要调用 `reloadInputViews` 才会生效。
3. **键盘区域**
   - 视图监听 `UIKeyboardWillChangeFrameNotification` 和 `UIKeyboardWillHideNotification`，记下键盘和窗口重叠部分的高度。
   - `inner_size()`/`inner_position()` 报告的安全区把这部分也去掉（底部取 max(安全区底边, 键盘重叠)），并发一次 `WindowEvent::Resized`（尺寸是不变的整屏尺寸），让应用重新读取。这个行为写进 fork 的文档。
   - 我的决定：不另加事件类型。fenestra 第一阶段已经按 inner 矩形算安全区并让出布局，"焦点滚进视野"会把文本框滚上来，fenestra 几乎不用改。
4. **间接滚动**
   - 加一个只接收间接滚动的 UIPanGestureRecognizer：`allowedScrollTypesMask = .all`、`allowedTouchTypes = []`，不干扰手指触摸。
   - 平移增量换算成 `WindowEvent::MouseWheel { delta: PixelDelta, phase }`：手势开始、变化、结束分别对应 Started、Moved、Ended。

### fenestra

- 新增 feature `ios-text-input`（fenestra-shell 和 fenestra 都有），用法和 `android-game-activity` 一样，只有打了 winit fork 补丁才能开。用到 fork 新接口的代码都放在 `cfg(all(target_os = "ios", feature = "ios-text-input"))` 下。不开这个 feature 时，iOS 行为和第一阶段一样。
- 开了这个 feature 时：
  - **文本同步**：`sync_ime_text` 在 iOS 上也编译，推送当前文本框的状态（复用 `ime_sync::ImeSync` 判断是否需要推送、过滤过期回声）。`Ime::TextState` 在 iOS 上也接收，走 `InputEvent::ImeState`。
  - **特征映射**：新增 `ime_sync::text_input_traits(&ImeTextState) -> TextInputTraits`，映射规则见目标 3，和 `editor_info` 对齐。换了文本框或特征变了才重新推送。
  - **光标区域**：每帧把聚焦文本框的光标矩形通过 `set_ime_cursor_area` 告诉 winit，用于候选词窗口定位和 `firstRectForRange`。
- **键盘升起时滚进视野**："焦点滚进视野"现在只在整窗尺寸变化时触发，键盘区域只改变安全区，整窗尺寸不变。要让安全区变化（即应用拿到的布局尺寸变化）也触发，否则滚动视图里的文本框不会被滚上来。wechat-ui 的聊天输入框固定在底部，不受影响，但其它应用会。（审阅后补充）
- **自动大写与自动更正**：`ImeTextState` 不带这两项，由键盘类型和 `masked` 推出：密码、邮箱、网址、数字类关闭；普通文本开启。
- **触控板滚动**：fenestra 已经处理 MouseWheel 像素增量，触摸路径（`touch.rs`）不受影响，不需要改。
- **触感**：新增 `WindowOptions::with_ios_haptic(fn(Haptic))`，长按产生应用消息时调用，复用 Android 的判定（`touch_haptic`）。这部分不依赖 fork，不放在 feature 下。

### wechat-ui

- `native/Cargo.toml` 和 `native/android/Cargo.toml` 都指向 fork 的同一个新提交，两个平台用同一份 winit。fork 包含 0.30.13 的全部内容，桌面不受影响。iOS 构建开启 `fenestra/ios-text-input`。
- iOS 触感钩子：用 objc2 调用 `UIImpactFeedbackGenerator(style: .medium).impactOccurred()`，放在只在 iOS 编译的模块里，和 Android 的 JNI 钩子对称。

## 怎么验证

- **winit fork 主机测试**：偏移换算（字节和 UTF-16，含中文和 emoji），镜像上的编辑操作（替换区间、设置和取消组字、删除），键盘重叠的计算。凡是不依赖 UIKit 的逻辑都抽成纯函数测试。
- **fenestra 主机测试**：`text_input_traits` 映射表（和 `editor_info` 的测试一一对应），iOS 推送节流沿用 `ImeSync` 的测试。
- **模拟器验证**（iPhone 17，iOS 26.5，不带临时补丁）：
  1. 点聊天输入框，键盘弹出，输入框在键盘上方；收起键盘后布局恢复。
  2. 用拼音键盘打"nihao"，选"你好"上屏；退格删除；发送后消息出现在列表里。
  3. 搜索框弹出时回车键显示"搜索"，聊天输入框显示"发送"，数字字段显示数字键盘（wechat-ui 若没有数字字段，用 fenestra 示例验证）。
  4. 触控板双指滑动聊天列表能滚动。
  5. 长按会话：钩子被调用（看日志；模拟器不会振动）。
- **Android 回归**：手机连接后在一加 6 上验证。fork 改动只在 iOS 平台代码里，Android 只需确认 IME 照常工作。

## 交付顺序

- 开发和大部分模拟器验证用本地 path 补丁，同时指向本地的 winit fork 和 fenestra。
- fenestra 照旧直接推送到 main（之前已授权）。
- winit fork 的推送需要用户同意。推送后 wechat-ui 改成指向 fork 的提交，再做一次不带补丁的最终验证。

## 我替用户做的决定

- 用"文本状态镜像 + `Ime::TextState`"实现 iOS 输入法，和 Android 走同一套同步逻辑，而不是 winit 上游的 Preedit/Commit 模型。
- 键盘区域并进 winit 报告的安全区，不新增事件类型。
- 键盘动画不跟随，布局直接跳到最终位置。
- 回车键在没有组字时变成 Enter 按键，不写进文本。
- 推送 winit fork 是对外操作，要先经用户同意。
