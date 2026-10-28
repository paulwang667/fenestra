# 输入框的键盘类型和回车键 设计

## 为什么做

在手机上（一加 6、Gboard），fenestra 的每个单行输入框都告诉输入法"普通文本、动作是完成"，多行输入框也一样。所以：

- 聊天输入框的键盘右下角不是"发送"，而是"完成"或换行；
- 搜索框没有搜索键；
- 数字、电话、邮箱、网址字段弹出的都是全键盘；
- 登录表单按回车不会跳到下一个输入框。

这是手机聊天体验最直接的缺口。用户在 2026-10-28 让我按建议开始做（"按你的建议开始"），流程由我自行推进。

## 目标

1. 应用能给输入框指定**键盘类型**：文本（默认）、整数、小数、电话、邮箱、网址。
2. 应用能给输入框指定**回车键**：默认、发送、搜索、前往、完成、下一项、换行。
3. Android 上输入法按这两项显示对应的键盘和右下角按键（Gboard 上可见"发送"、搜索图标、数字键盘等）。
4. "下一项"在所有平台上都有效：在这种输入框里按回车（手机上就是按输入法的"下一项"），焦点移到下一个可聚焦元素，和 Tab 一样。
5. wechat-ui 的聊天输入框显示"发送"，搜索框显示搜索键。

## 不做什么

- **iOS 的键盘类型**：winit 没有接口，不在本次范围。iOS 上这两项暂时不起作用（字段照常输入）。
- **"上一项"**：Android 上拿不到 Shift 修饰键，不做。
- **自动大写、自动更正这类文本标志**：不做。
- 改动我们自己的 winit 分支：不需要（所有动作键本来就以回车送达）。

## 反目标（做了也算失败的情况）

- 没设置这两项的应用，行为有任何变化：单行字段仍是"文本 + 完成"，多行字段回车仍换行，密码字段仍不给建议、不学习。
- 输入法组合（拼音）过程中，因为反复推送编辑器信息而丢失正在输入的拼音。
- 桌面上普通单行字段按回车也开始跳焦点（只有明确设置了"下一项"的字段才跳）。

## 怎么做（已定的部分）

### core

- 新增 `pub enum Keyboard { Text, Number, Decimal, Phone, Email, Url }`（默认 `Text`）和 `pub enum EnterKey { Default, Send, Search, Go, Done, Next, Newline }`（默认 `Default`）。
- `InputData` 加 `keyboard`、`enter_key` 两个字段；输入元素加 `.keyboard(k)`、`.enter_key(k)` 两个构建方法（和 `.masked()` 同样的写法）。
- `ImeTextState` 加 `keyboard: Keyboard`、`enter_key: EnterKey`、`multiline: bool`，由聚焦字段填入。`enter_key` 在这里是**解析后**的值：`Default` 在多行且未开 `submit_on_enter` 时解析为 `Newline`，否则解析为 `Done`（即现在的行为）。
- **回车键决定编辑器怎么处理回车**（解析后的值）：`Newline` → 编辑器插入换行（单行字段上 `Newline` 解析为 `Done`）；其他任何值 → 编辑器不插入换行，回车交给应用（多行字段上 Shift+回车仍换行，和现在的 `submit_on_enter` 一样）。`submit_on_enter` 只影响 `Default` 的解析，因此显式值和它冲突时以显式值为准。
- **下一项**：聚焦的输入框 `enter_key == Next` 时收到回车（`Key::Enter`，没有修饰键，且不在拼音组合中），先照常交给元素的 `on_key`（应用处理了就到此为止）；没人处理时，焦点按 Tab 的顺序移到下一个可聚焦元素（和 Tab 一样在末尾绕回）。
- **完成 / 搜索 / 前往**：**显式**设了这三者之一的**单行**字段，回车没人处理时清除焦点（手机上键盘随之收起，这是 Android 的惯例）。只影响显式设置的字段；默认解析出的 `Done` 不失焦，行为不变。发送不失焦（聊天输入框发完还要继续输入）。

### shell（Android，GameActivity）

- 一个与平台无关、可在主机上测试的映射函数，把 `(keyboard, enter_key, multiline, masked)` 转成 EditorInfo 的三个数值：`inputType`、动作、`imeOptions`。规则：
  - 键盘类型：文本 → `TYPE_CLASS_TEXT`；整数 → `TYPE_CLASS_NUMBER`；小数 → `TYPE_CLASS_NUMBER | TYPE_NUMBER_FLAG_DECIMAL`；电话 → `TYPE_CLASS_PHONE`；邮箱 → `TEXT | TYPE_TEXT_VARIATION_EMAIL_ADDRESS`；网址 → `TEXT | TYPE_TEXT_VARIATION_URI`。
  - 密码：文本类 → `TEXT | VARIATION_PASSWORD | FLAG_NO_SUGGESTIONS`；数字类 → `NUMBER | NUMBER_VARIATION_PASSWORD`；两者都加 `IME_FLAG_NO_PERSONALIZED_LEARNING`（保持现在密码字段的做法）。
  - 多行字段（文本类）总是带 `TYPE_TEXT_FLAG_MULTI_LINE`。
  - 回车键：发送/搜索/前往/完成/下一项 → 对应的 `IME_ACTION_*`，不加 `IME_FLAG_NO_ENTER_ACTION`（输入法显示动作键；多行 + 发送的聊天输入框在手机上因此不能输入换行，和微信一样）；换行 → `IME_ACTION_NONE` 加 `IME_FLAG_NO_ENTER_ACTION`。
  - 所有情况都加 `IME_FLAG_NO_FULLSCREEN`（保持现状）。
- `sync_ime_text` 现在只在"是否密码"变化时调用 `set_ime_editor_info`；改为在这四项（映射结果）与输入法当前持有的不同时调用。只在有差异时推送，避免打断正在进行的拼音组合（推送总是在 `set_text_input_state` 之前，和现在一样）。

### kit

- `text_input` 增加 `.keyboard()` / `.enter_key()`，透传到元素；再加 `.on_submit(msg)`：回车（含手机上的动作键）时发出这条消息。

### wechat-ui

- 聊天输入框：`EnterKey::Send`。
- 会话搜索框：`EnterKey::Search`（结果本来就随输入实时过滤，按搜索键只收起键盘）。

## 怎么验证

- core：`ImeTextState` 带出键盘类型、解析后的回车键、多行；`Default` 的解析规则；`Next` 字段回车跳到下一个字段；应用的 `on_key` 处理了回车时不跳；没设置的单行字段回车不跳。
- shell：映射函数对每种组合给出预期的三个数值（含密码、多行、默认）。
- 真机（一加 6，Gboard）：聊天输入框右下角是"发送"，按下就发出消息；搜索框是搜索键；临时演示页里数字/电话/邮箱字段弹出对应键盘；输入拼音中途不丢字。截图存笔记目录。

## 留给实现者

- 枚举放在哪个模块、是否 `#[non_exhaustive]`（建议加，以后可能增加类型）。
- `ImeSync` 里记录"上次推送的编辑器信息"的方式。

## 我替用户做的决定

- 显式的完成/搜索/前往在回车没人处理时失焦（收起键盘）；发送和默认解析出的完成不失焦。
- 显式回车键决定编辑器行为，`submit_on_enter` 只影响默认值。
- 多行字段总带 MULTI_LINE；设了动作键的多行字段在手机上不能输入换行。
- "下一项"在所有平台生效（不只是手机），但只对明确设置了它的字段。
- iOS 暂不支持。
- 默认值保持现在的行为（单行"完成"、多行换行）。
