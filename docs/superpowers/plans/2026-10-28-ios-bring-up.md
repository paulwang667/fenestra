# iOS 第一阶段（模拟器打通）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 不带任何补丁，fenestra 应用（wechat-ui）在 iOS 模拟器上启动即是可用的手机界面。

**Architecture:** fenestra-shell 新增自管 GPU 模块替换 `vello::util`（设备限制按适配器下调）；后端选择、窗口属性、可见性、致命错误、软键盘按 iOS 调整；安全区由 shell 在主窗口根上包一层带内边距和背景的元素实现，绘制、玻璃和输入坐标不用改。wechat-ui 加模拟器打包脚本。

**Tech Stack:** Rust、winit 0.30.13、wgpu 29、vello 0.9、Xcode 模拟器（iOS 26.5，iPhone 17）。

**Spec:** `docs/superpowers/specs/2026-10-28-ios-bring-up-design.md`；打通实验记录 `side-session-notes/local_3aaa63c9-.../android-ime-spike/IOS-SPIKE.md` 与 `ios-spike-fenestra.diff`。

## Global Constraints

- fenestra `unsafe_code = "forbid"`，不加 objc/JNI 依赖。
- 非 iOS 平台行为不变（安全区恒为零；Android 软键盘逻辑不变）。
- 不跑 `cargo test --workspace`；共享 `CARGO_TARGET_DIR=.../scratchpad/target`，`CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0`；推送前跑 `scratchpad/fullcheck.sh`。
- 推送 fenestra main 已授权（`paulwang667/fenestra`）；wechat-ui 只本地提交，不跑 `cargo fmt`。
- iOS 构建目标：`aarch64-apple-ios-sim`；包名 `dev.fenestra.wechatui`。

## Review Focus

1. 后台再回前台（suspend/resume）：iOS 上表面重建后，尺寸仍是整屏、安全区仍正确、键盘不会自己弹出。→ Task 4、5 的模拟器验证步骤覆盖（Home 键后重新打开）。
2. 安全区在运行中变化（`Resized`）：重新读取并重新布局，不沿用旧的帧缓存。→ Task 4 测试 `changed_insets_rebuild_the_frame`。
3. 用户手动收起键盘后再点同一个输入框：键盘重新弹出。→ Task 5 测试 `a_tap_reshows_a_dismissed_keyboard`。
4. 换 GPU 模块后无头渲染（金样测试）像素不变。→ Task 1 跑所有依赖无头渲染的包。
5. 单行输入框里按软键盘的换行键：触发回车动作（搜索框的搜索、发送），不插入换行字符。→ Task 5 测试 `a_newline_text_is_the_enter_key`。

---

### Task 1: 自管 GPU 模块

**Files:**
- Create: `fenestra-shell/src/gpu.rs`
- Modify: `fenestra-shell/src/lib.rs`（`mod gpu;`）、`fenestra-shell/src/window.rs`（`use vello::util::{RenderContext, RenderSurface}` 及所有用法，含 wasm 的 `activate`/`pump`）、`fenestra-shell/src/headless.rs`

**Interfaces:**
- Produces（`pub(crate)`，字段名与 `vello::util` 相同，调用处只换类型名）：
  - `struct GpuContext { pub instance: wgpu::Instance, pub devices: Vec<DeviceHandle> }`，`fn new() -> Self`，`async fn device(&mut self, compatible: Option<&wgpu::Surface<'_>>) -> Option<usize>`，`async fn create_surface<'w>(&mut self, window: impl Into<wgpu::SurfaceTarget<'w>>, width: u32, height: u32, present_mode: wgpu::PresentMode) -> Result<GpuSurface<'w>, vello::Error>`，`fn resize_surface(&self, surface: &mut GpuSurface<'_>, width: u32, height: u32)`
  - `struct DeviceHandle { adapter: wgpu::Adapter, pub device: wgpu::Device, pub queue: wgpu::Queue }`，`fn adapter(&self) -> &wgpu::Adapter`
  - `struct GpuSurface<'s> { pub surface, pub config, pub dev_id, pub format, pub target_texture, pub target_view, pub blitter }`（类型同 vello 0.9 `RenderSurface`）
  - `fn device_limits(adapter: &wgpu::Limits) -> wgpu::Limits` = `wgpu::Limits::default().or_worse_values_from(adapter)`
- 逻辑照搬 vello 0.9 `src/util.rs`（实例、`initialize_adapter_from_env_or_default`、可选特性 `CLEAR_TEXTURE | PIPELINE_CACHE` 取交集、表面格式选择、中间纹理与 `TextureBlitter`），文件头注明来源与许可（vello 为 Apache-2.0 OR MIT），唯一行为差别是 `required_limits: device_limits(&adapter.limits())`。

- [ ] **Step 1: 写失败测试**（`gpu.rs` 内 `#[cfg(test)]`）

```rust
#[test]
fn limits_drop_to_what_the_adapter_offers() {
    let adapter = wgpu::Limits { max_inter_stage_shader_variables: 15, ..wgpu::Limits::default() };
    let got = device_limits(&adapter);
    assert_eq!(got.max_inter_stage_shader_variables, 15);
    assert_eq!(got.max_storage_buffers_per_shader_stage, wgpu::Limits::default().max_storage_buffers_per_shader_stage);
}

#[test]
fn a_stronger_adapter_still_gets_the_defaults() {
    let adapter = wgpu::Limits { max_inter_stage_shader_variables: 31, max_texture_dimension_2d: 16384, ..wgpu::Limits::default() };
    assert_eq!(device_limits(&adapter), wgpu::Limits::default());
}
```

- [ ] **Step 2:** `cargo test -p fenestra-shell --lib gpu::` → 编译失败（`device_limits` 未定义）。
- [ ] **Step 3:** 实现 `gpu.rs`，把 `window.rs`、`headless.rs` 的 `RenderContext`/`RenderSurface` 换成 `GpuContext`/`GpuSurface`；`composite_with_alpha` 等签名随之改类型。
- [ ] **Step 4:** `cargo test -p fenestra-shell --lib gpu::` → PASS；`cargo check -p fenestra-shell --target wasm32-unknown-unknown` 与 `--target aarch64-linux-android --features android-game-activity`（按 fullcheck.sh 现有写法）通过。
- [ ] **Step 5:** 逐包跑依赖无头渲染的测试：`cargo test -p fenestra-shell`、`-p fenestra`、`-p fenestra-kit`、`-p fenestra-looks`、`-p fenestra-charts`、`-p fenestra-markdown`（以 `cargo tree -i fenestra-shell` 列出的工作区包为准）→ 全部通过，金样无差异。
- [ ] **Step 6: Commit** `refactor(shell): own GPU device and surface setup; cap limits to the adapter`

### Task 2: 后端选择

**Files:** Modify `fenestra-shell/src/backend.rs`、`fenestra-shell/src/window.rs:680-688`（`activate` 里的后端判定）

**Interfaces:**
- Consumes: Task 1 的 `DeviceHandle::adapter()`。
- Produces: `pub(crate) fn window_backend(requested: Backend, adapter: wgpu::Backend, indirect: bool, cpu: bool) -> Backend`；`PLATFORM_DEFAULT` 在 `target_os = "ios"` 也为 `Raster`。`effective_backend` 保留给无头渲染（显式 `with_backend` 不回退）。

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn vello_without_indirect_dispatch_falls_back_to_raster() {
    assert_eq!(window_backend(Backend::Vello, wgpu::Backend::Metal, false, false), Backend::Raster);
}
#[test]
fn vello_keeps_vello_when_it_can_run() {
    assert_eq!(window_backend(Backend::Vello, wgpu::Backend::Metal, true, false), Backend::Vello);
    // CPU 模式不需要间接派发
    assert_eq!(window_backend(Backend::Vello, wgpu::Backend::Metal, false, true), Backend::Vello);
}
#[test]
fn neither_backend_fitting_keeps_vello() {
    assert_eq!(window_backend(Backend::Vello, wgpu::Backend::Gl, false, false), Backend::Vello);
    assert_eq!(window_backend(Backend::Raster, wgpu::Backend::Gl, true, false), Backend::Vello);
}
```

- [ ] **Step 2:** `cargo test -p fenestra-shell --lib backend::` → 编译失败。
- [ ] **Step 3:** 实现 `window_backend`（先套用现有 `effective_backend`，再加 vello→raster 规则）；`activate` 用它，`indirect = adapter.get_downlevel_capabilities().flags.contains(DownlevelFlags::INDIRECT_EXECUTION)`，`cpu = crate::headless::cpu_compute_requested()`；发生 vello→raster 时打印一次 `fenestra: this adapter cannot run vello's compute pipeline (no indirect dispatch); rendering with raster`。`PLATFORM_DEFAULT` 改为 `cfg!(any(target_os = "android", target_os = "ios"))`。
- [ ] **Step 4:** `cargo test -p fenestra-shell --lib backend::` → PASS。
- [ ] **Step 5: Commit** `feat(shell): raster by default on iOS; fall back from vello without indirect dispatch`

### Task 3: iOS 窗口：可见性、窗口属性、致命错误

**Files:** Modify `fenestra-shell/src/window.rs`（`resumed_with` 540-630、`fail` 529）

**Interfaces:**
- Produces: `fn needs_show(visible: Option<bool>) -> bool`；`struct Geometry { inner_size: Option<(f64, f64)>, min_size: Option<(f64, f64)>, position: Option<(f64, f64)>, maximized: bool, fullscreen: bool }`，`fn window_geometry(options: &WindowOptions, mobile: bool) -> Geometry`（`mobile = cfg!(target_os = "ios")`；Android 保持现状，不在本任务改）。

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn a_window_of_unknown_visibility_is_shown() {
    assert!(needs_show(None));
    assert!(needs_show(Some(false)));
    assert!(!needs_show(Some(true)));
}
#[test]
fn a_mobile_window_ignores_desktop_geometry() {
    let o = WindowOptions::titled("t").with_min_size(960.0, 640.0).with_position(10.0, 20.0).fullscreen();
    let g = window_geometry(&o, true);
    assert_eq!((g.inner_size, g.min_size, g.position, g.maximized, g.fullscreen), (None, None, None, false, false));
    let d = window_geometry(&o, false);
    assert_eq!(d.min_size, Some((960.0, 640.0)));
    assert_eq!(d.position, Some((10.0, 20.0)));
}
```

- [ ] **Step 2:** `cargo test -p fenestra-shell --lib needs_show a_mobile_window` → 编译失败。
- [ ] **Step 3:** 实现并接入 `resumed_with`（属性按 `Geometry` 设置；`was_hidden` 改为 `needs_show(window.is_visible())`）。`fail()`：`#[cfg(target_os = "ios")] panic!("fenestra: {err}")`（iOS 的 `run_app` 不返回，没人读 `fatal`），其它平台不变。
- [ ] **Step 4:** 测试 PASS；`cargo check -p fenestra-shell --target aarch64-apple-ios-sim` 通过。
- [ ] **Step 5: Commit** `fix(shell): show the window on iOS, open it full screen, and surface startup failures`

Ruling（预先记录）：`fail()` 的 iOS panic 只能在模拟器上验证，主机上无测试——它是一个 cfg 分支，没有可测的逻辑。

### Task 4: 安全区

**Files:** Create `fenestra-shell/src/safe_area.rs`；Modify `fenestra-shell/src/window.rs`（`activate`/`resized` 的表面尺寸、`AppRunner::redraw` 2397 起）

**Interfaces:**
- Produces:
  - `#[derive(Clone, Copy, Debug, Default, PartialEq)] pub(crate) struct Insets { top: f32, right: f32, bottom: f32, left: f32 }`（逻辑像素）
  - `fn insets_from(outer: (u32, u32), inner_pos: (i32, i32), inner: (u32, u32), scale: f64) -> Insets`（物理→逻辑；负值钳到 0）
  - `fn window_insets(window: &Window) -> Insets`：iOS 用 `outer_size`/`inner_position`/`inner_size` 算；其它平台 `Insets::default()`
  - `fn inset_root<M>(view: Element<M>, insets: Insets, fallback_bg: Color) -> Element<M>`：零安全区原样返回；否则包一层 `w_full().h_full()`、`pt/pr/pb/pl` = 安全区、背景 = `view` 根的背景（`view.style()` 上有背景时）否则 `fallback_bg`
  - `fn surface_size(window: &Window) -> PhysicalSize<u32>`：iOS 用 `outer_size()`，其它 `inner_size()`
- `redraw`：`insets = window_insets(..)`；`view_at` 收到 `(lw - left - right, lh - top - bottom)`；`build_frame` 仍按整屏 `(lw, lh)`，传 `inset_root(view, insets, theme.bg)`。`insets` 进入帧缓存键（`raster_key` / `cached_scene` 的键），变化即重建。输入坐标不变（帧覆盖整屏）。

- [ ] **Step 1: 写失败测试**（`safe_area.rs`）

```rust
#[test]
fn no_safe_area_is_zero_insets() {
    assert_eq!(insets_from((1206, 2622), (0, 0), (1206, 2622), 3.0), Insets::default());
}
#[test]
fn a_notched_phone_insets_top_and_bottom() {
    // iPhone 17：顶部 62pt、底部 34pt，@3x
    let i = insets_from((1206, 2622), (0, 186), (1206, 2622 - 186 - 102), 3.0);
    assert_eq!(i, Insets { top: 62.0, right: 0.0, bottom: 34.0, left: 0.0 });
}
#[test]
fn the_view_sits_inside_the_insets_and_the_root_fills_the_screen() {
    // build_frame(inset_root(col().id("app").bg(RED).w_full().h_full(), Insets{top:62,..bottom:34}, BLUE), 402×874)
    // 断言：id "app" 的 rect == (0, 62, 402, 778)；包装根 rect == (0,0,402,874) 且背景为 RED（取自 view 根）
}
#[test]
fn a_view_without_background_falls_back_to_the_theme() { /* 同上，view 无背景 → 包装根背景 == BLUE */ }
#[test]
fn zero_insets_leave_the_view_alone() { /* inset_root 后根仍是 id "app" */ }
#[test]
fn changed_insets_rebuild_the_frame() { /* 帧缓存键函数：同尺寸不同 insets → 不复用 */ }
```

（最后一条针对 `reuses_raster_frame(dirty, rendered, now, target, surface)`：把键从 `(lw, lh, scale)` 扩成含 `Insets` 的元组，相同尺寸、不同 insets 时返回 false；`cached_scene` 的键同步扩展。）

- [ ] **Step 2:** `cargo test -p fenestra-shell --lib safe_area::` → 编译失败。
- [ ] **Step 3:** 实现并接入 `redraw`、`activate`（原生 `activate` 与 `resized` 用 `surface_size`）。
- [ ] **Step 4:** 测试 PASS；`cargo test -p fenestra-shell` 全过；`cargo check --target aarch64-apple-ios-sim -p fenestra-shell`。
- [ ] **Step 5: Commit** `feat(shell): lay the main window out inside the iOS safe area`

### Task 5: 软键盘与回车

**Files:** Modify `fenestra-shell/src/window.rs`（`sync_soft_keyboard` 2745、`keyboard_shown`/`keyboard_tap` 字段与赋值处的 cfg、`resumed` 3368-3378、KeyboardInput 3777）

**Interfaces:**
- Produces: `fn keyboard_change(want: bool, shown: bool, tapped: bool) -> Option<bool>`（返回要设置的 `set_ime_allowed` 值，`None` 表示不动）；`fn text_as_key(text: &str) -> Option<KeyInput>`（`"\n"`、`"\r"` → Enter，无修饰键）。

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn the_keyboard_follows_the_caret() {
    assert_eq!(keyboard_change(true, false, false), Some(true));
    assert_eq!(keyboard_change(false, true, false), Some(false));
    assert_eq!(keyboard_change(false, false, true), None);
}
#[test]
fn a_tap_reshows_a_dismissed_keyboard() {
    assert_eq!(keyboard_change(true, true, true), Some(true));
    assert_eq!(keyboard_change(true, true, false), None);
}
#[test]
fn a_newline_text_is_the_enter_key() {
    assert_eq!(text_as_key("\n").map(|k| k.key), Some(Key::Enter));
    assert_eq!(text_as_key("a"), None);
}
```

（`KeyInput`/`Key` 用 fenestra-core 里 `map_key` 产出的同一类型，按其实际字段名写断言。）

- [ ] **Step 2:** 运行 → 编译失败。
- [ ] **Step 3:** `sync_soft_keyboard` 用 `keyboard_change`，cfg 改为 `any(target_os = "android", target_os = "ios")`（字段、`keyboard_tap = true` 处、调用处同步）；`resumed` 里"无条件 `set_ime_allowed(true)`"排除 iOS，iOS 走 `keyboard_shown = false`。KeyboardInput 分支：非 printable 时先试 `event.text` 的 `text_as_key`（winit iOS 把换行键作为 `insertText("\n")` 发出）。
- [ ] **Step 4:** 测试 PASS；`cargo test -p fenestra-shell`；android 与 ios-sim 目标 `cargo check` 通过。
- [ ] **Step 5: Commit** `feat(shell): the iOS keyboard follows focus; its return key is Enter`

### Task 6: 门控、推送

- [ ] CHANGELOG（Added：iOS 模拟器支持、安全区；Fixed：iOS 窗口不显示、启动失败静默、模拟器无 GPU 设备、vello 无间接派发 panic）。
- [ ] `scratchpad/fullcheck.sh ios-bring-up` 全过（含 Task 1 Step 5 的逐包测试）。
- [ ] 推送 fork main，`git update-ref refs/remotes/fork/main main`，`gh workflow run CI --repo paulwang667/fenestra --ref main`。

### Task 7: wechat-ui 与验证

**Files:** Create `native/ios/Info.plist`、`native/ios/run-sim.sh`；Modify `native/Cargo.lock`（`cargo update -p fenestra`）、`native/android/Cargo.lock`

- [ ] `Info.plist`：`CFBundleIdentifier dev.fenestra.wechatui`、`CFBundleExecutable wechat-fui-native`、`CFBundleName WeChatUI`、`CFBundlePackageType APPL`、版本 0.1/1、`UILaunchScreen {}`、`UIRequiredDeviceCapabilities [arm64]`、`UISupportedInterfaceOrientations [Portrait]`、`CFBundleDevelopmentRegion zh_CN`（字符串）。
- [ ] `run-sim.sh [device]`（默认 `iPhone 17`）：`cargo build --target aarch64-apple-ios-sim --bin wechat-fui-native`（沿用 `CARGO_TARGET_DIR`，未设置时用 cargo 默认）→ 在 `$TARGET_DIR/aarch64-apple-ios-sim/debug/WeChatUI.app` 组装 → `codesign -f -s -` → 必要时 `simctl boot` → `simctl install` → `simctl launch --console-pty --terminate-running-process`；当前环境里的 `FENESTRA_*`/`WECHATUI_*` 以 `SIMCTL_CHILD_` 前缀转给应用。文件头注释说明只用于模拟器。
- [ ] `cargo test`（native）通过；本地提交 `build: bump fenestra (iOS bring-up); simulator run script`。
- [ ] 模拟器验证（不带补丁）：首页顶部栏在状态栏下、底部标签栏在横条上、状态栏区为背景色；启动无键盘；点搜索框弹键盘、打 `abc` 退格一次得 `ab`、按换行键执行搜索动作、点别处收起；进聊天页、返回、惯性滚动、长按菜单；按 Home 后重新打开，布局与键盘状态正确。截图存笔记目录；记录"键盘盖住聊天输入框"为已知缺陷。
- [ ] Android 回归（一加 6）：启动、滚动、键盘、帧时间与之前一致；恢复 Gboard 与 `debug.wechatui.*`。

### Task 8: 审查

- [ ] 独立 opus 审查整个改动（fenestra 与 wechat-ui），按 Review Focus 逐条检查；Critical/Important 一轮修复，每项先写失败测试；Minor 记入账本。
- [ ] 笔记目录更新 IOS-SPIKE.md 的结论与账本。
