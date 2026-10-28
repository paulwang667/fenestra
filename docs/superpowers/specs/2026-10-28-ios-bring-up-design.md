# iOS 第一阶段：在模拟器上正常运行 设计

## 为什么做

用户在 2026-10-28 选择下一步做"iOS 支持"，流程由我自行推进。同日的打通实验（wechat-ui 桌面入口直接编进 `aarch64-apple-ios-sim`，手工 `.app`，iPhone 17 模拟器 iOS 26.5）证明 fenestra 的布局、文字、触摸、raster 渲染在 iOS 上都能工作，但要绕过 7 个 fenestra 缺陷才能看到画面。实验记录：`side-session-notes/.../android-ime-spike/IOS-SPIKE.md`，临时补丁 `ios-spike-fenestra.diff`。

第一阶段的目标是：**不带任何补丁，fenestra 应用在 iOS 模拟器上启动即是可用的手机界面**。

## 目标

1. 窗口启动后可见，不再黑屏。
2. 启动失败时，错误信息出现在控制台（`simctl launch --console-pty` 能看到），不再是静默黑屏。
3. 在 iOS 模拟器上能建出 GPU 设备（模拟器 `max_inter_stage_shader_variables` 只有 15）。
4. iOS 默认用 raster 后端；任何平台上，要求 vello 而适配器缺少 vello 计算管线需要的能力（`DownlevelFlags::INDIRECT_EXECUTION`）时，自动退到 raster 并提示一次，而不是 panic。
5. 窗口铺满屏幕：iOS 上忽略应用给的窗口尺寸、最小尺寸、位置（这些是桌面概念）。
6. 内容在安全区内：状态栏、灵动岛、底部横条下面不放内容；安全区外用根元素的背景色填满，看起来是一整块屏幕。触摸坐标与布局一致。
7. 软键盘跟随焦点：只在文本框持有光标时弹出，失焦时收起（和 Android 一样），启动时不弹。
8. wechat-ui 有一键脚本：构建、打包 `.app`、安装并在模拟器启动。

## 不做什么（第二阶段，需要 winit fork）

- 键盘类型与回车键（UITextInputTraits）、中文输入法的组字（UITextInput 标记文本）。
- **键盘遮挡**：iOS 弹出键盘时不缩小窗口，winit 也不报告键盘高度，所以聊天页输入框会被键盘盖住。第一阶段接受这个已知缺陷，明确写进验证结果；第二阶段在 winit fork 里报告键盘区域，复用"焦点滚进视野"的机制。
- 触感（UIImpactFeedbackGenerator 钩子，仿照 `with_android_haptic`）。
- 真机（需要签名与开发者账号）、横屏、iPad、全面屏"内容延伸到边缘"（应用自己在状态栏下画东西）。
- Xcode 工程、App Store 打包。

## 怎么做

### fenestra-shell

1. **可见性**：创建窗口后，`is_visible()` 不是 `Some(true)` 就调用 `set_visible(true)`（iOS 永远返回 `None`；Wayland、Android 上 `set_visible` 无害）。
2. **致命错误**：在 `run_app` 不返回的平台（iOS），`fail()` 直接以错误信息 panic，让信息进控制台和崩溃报告。其它平台行为不变（存进 `fatal`、退出事件循环、由 `run_app` 返回）。
3. **GPU 设备**：fenestra 不再用 `vello::util::RenderContext` / `RenderSurface`，改为 shell 内自己的小模块（实例、按适配器建设备、建/调整表面、vello 需要的中间纹理与 blit）。设备限制 = `Limits::default()` 按适配器能力下调（`or_worse_values_from(&adapter.limits())`），可选特性与现在相同（`CLEAR_TEXTURE`、`PIPELINE_CACHE` 取交集）。原生窗口、无头渲染（`headless.rs`）、web 都换到这个模块。
   - 我的决定：不走"给 vello 上游提 PR 再等发布"——`DeviceHandle.adapter` 是私有字段，下游无法注入设备，而 `[patch]` 不会传给 fenestra 的用户。vello 的 `util` 只是便利层，自管代价小。
4. **后端选择**：`PLATFORM_DEFAULT` 在 iOS 上也是 `Raster`。`effective_backend` 增加一条：要求 vello、设备不支持 `INDIRECT_EXECUTION`、**且 raster 在该适配器上可用（Vulkan/Metal/DX12）、且没有开 `FENESTRA_CPU`** 时退到 raster（沿用现有一次性提示的方式）。两条规则放在同一个函数里。
   - 两个后端都不合适（GL 适配器缺间接执行）时保持 vello，行为与现在相同（我的决定：不新增错误路径）。
   - 只作用于窗口按环境变量/平台默认选出的后端；无头渲染里显式 `with_backend(Backend::Vello)` 不回退，避免 vello 金样测试在弱适配器上悄悄换成 raster。
5. **iOS 窗口属性**：iOS 上不设置 `inner_size`、`min_inner_size`、`position`、`maximized`、`fullscreen`（winit 会把 `inner_size` 当作 UIWindow 的 frame）。
6. **安全区**（只在 iOS 生效，其它平台的安全区恒为零；只作用于 `run_app` 的主窗口——iOS 应用只有这一个窗口。`run_static`、`run_scene`、副窗口、`Embedded` 第一阶段不处理，安全区为零）：
   - 表面用整屏尺寸（`outer_size`）创建和调整。winit iOS 的 `inner_size()` / `inner_position()` 报告的是安全区。
   - 每帧布局的视口 = 安全区（逻辑像素），`view_at` 收到的尺寸是安全区尺寸，因此 wechat-ui 的 `MOBILE_MAX_WIDTH` 判断照常工作。
   - 绘制时整棵树平移安全区原点；安全区外用根元素的背景色填充（根元素没有背景时用主题的窗口背景）。
   - 输入：触摸和指针坐标减去安全区原点再分发。
   - 安全区变化（旋转、状态栏变化）时重新布局；`Resized` 和 `ScaleFactorChanged` 时重新读取。
   - 安全区换算抽成可在主机上测试的纯函数（输入：整屏物理尺寸、安全区物理原点和尺寸、缩放；输出：逻辑视口原点与尺寸）。
7. **软键盘**：`sync_soft_keyboard` 及相关状态（`keyboard_shown`、`keyboard_tap`）在 Android 和 iOS 上编译；iOS 恢复时不再无条件 `set_ime_allowed(true)`。
8. **打字**：第一阶段必须能在聚焦的文本框里用软键盘输入英文和退格。winit 0.30 在 iOS 上把 `insertText`/`deleteBackward` 作为 `KeyboardInput` 发出（不是 `Ime::Commit`），确认这条路径在 iOS 上进到文本框，不通就补上。

### wechat-ui

- `native/ios/Info.plist`（包名 `dev.fenestra.wechatui`，竖屏，`UILaunchScreen`）和 `native/ios/run-sim.sh`：构建 `aarch64-apple-ios-sim`、组装 `.app`、ad-hoc 签名、`simctl install` 与 `launch --console-pty`；支持传入 `FENESTRA_*` / `WECHATUI_*` 环境变量（`SIMCTL_CHILD_` 前缀）。
- 入口沿用桌面 `src/main.rs`：fenestra 在 iOS 上忽略桌面窗口选项后，无需 iOS 专用入口。
- 打包脚本只用于模拟器；`.app` 产物放在 `CARGO_TARGET_DIR` 下，不进仓库。

## 交付顺序与门控

- fenestra 改动按流程推到 `paulwang667/fenestra` 的 main（已获授权），wechat-ui 更新锁文件后再做"不带补丁"的模拟器验证；wechat-ui 只本地提交。
- 门控：不跑 `cargo test --workspace`（磁盘预算）。用 `fullcheck.sh`：fmt、对改动的包 `clippy -p ... -D warnings`、`cargo deny check`、逐包 `cargo test -p`（关闭调试信息）。GPU 模块改动影响无头渲染，所以要逐包跑所有依赖 `fenestra-shell` 无头渲染的包（含金样测试）。

## 怎么验证

- fenestra 主机测试：
  - 安全区换算（零安全区等于整屏；有顶部/底部安全区时视口与原点正确；缩放 3 倍）。
  - 后端选择：iOS 默认 raster；缺 `INDIRECT_EXECUTION` 时从 vello 退到 raster。
  - 设备限制下调：适配器限制低于默认值时请求值取适配器值，其它取默认值。
  - 布局与输入偏移：带安全区时，点在安全区原点下方的元素能被点中，根元素背景填满整屏。
- 全量门控；Android 一加 6 回归：启动、滚动、键盘、帧时间与之前一致（Android 上安全区恒为零）。
- 模拟器（iPhone 17，iOS 26.5），**不带任何补丁**：
  - 启动后看到手机首页，顶部栏在状态栏下方，底部标签栏在横条上方，状态栏区域是背景色；
  - 启动时没有键盘；点搜索框弹出键盘，点别处收起；在搜索框里打英文和退格，文字正确；
  - 点会话进入聊天页、返回、惯性滚动、长按菜单；
  - 截图存入笔记目录；记录已知缺陷"键盘盖住输入框"。

## 我替用户做的决定

- 第一阶段只做模拟器，键盘遮挡、输入法、触感放第二阶段（都需要改 winit）。
- fenestra 自管 GPU 设备与表面，不依赖 vello 上游改动。
- 安全区由 shell 统一让出，应用无需改代码；不提供"内容延伸到状态栏下"的选项。
- iOS 默认 raster 后端（与 Android 一致；模拟器上 vello 计算管线不可用）。
