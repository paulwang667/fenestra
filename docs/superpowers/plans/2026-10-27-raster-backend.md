# 光栅化后端：fenestra-raster 实现 DrawTarget，接入 shell — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新增 `fenestra-raster` crate，用传统 wgpu 光栅化（lyon 三角化 + 4x MSAA + 模板裁剪 + 离屏图层 + 解析阴影 + 字形图集）实现 `fenestra_core::draw::DrawTarget`；shell 的窗口和无头渲染都能选用它，Android 默认用它。

**Architecture:** `RasterRenderer`（长期持有管线、字形图集、渐变表、图片缓存、纹理池）每帧开一个 `RasterFrame`，`RasterFrame` 实现 `DrawTarget`：绘制操作在 CPU 上变换、三角化成按图层分组的批次，`render()` 时按图层后序（子图层先画进离屏纹理）提交。shell 新增 `Backend { Vello, Raster }`（`FENESTRA_RENDERER`），在 `Headless::render_plan` 和窗口重绘两处分流：vello 走原来的 `Scene` 路径，一字不改；raster 走 `Frame::paint_*_into`。玻璃两遍渲染沿用现有的 CPU 滤镜，只是背景区域改由光栅化后端渲染、读回。正确性主要靠两种比对：每个图元和 vello 的输出做差分测试；整套现有快照用 `FENESTRA_RENDERER=raster` 再跑一遍。

**Tech Stack:** Rust 2024；wgpu 29.0.3；lyon_tessellation 1.0.23；etagere 0.3.0；vello_cpu 0.2.0（`text` 特性，仅用来把单个字形光栅化进图集）；skrifa 0.42.1；bytemuck 1.25；kurbo 0.13.1 / peniko 0.6.1（工作区版本）。

**Spec:** 笔记目录 `android-ime-spike/BACKEND-REPORT.md`："结论"一节和"光栅化后端接入 fenestra 的工作量"表格。原型代码是 `scratchpad/rbench/src/raster.rs`，数据在同一份报告的"更新 2"。本计划覆盖表格中的这些行：三角化（不含缓存）、字形图集、裁剪与图层、渐变/图片/阴影、快照适配与选后端。不覆盖：三角化缓存和缓冲区复用（第三份计划，性能）、GPU 玻璃模糊、GLES 兜底。

## Global Constraints

- 只在 `spike/android-game-activity-ime` 分支上提交（worktree `scratchpad/fen-wt`），不推送，不动 `main`。
- `unsafe_code = "forbid"`。公开 API 里不能有过程宏，新增的 `pub` 项都要写 doc 注释。按仓库 CLAUDE.md：能修好的警告不许用 `#[allow]` 压掉。
- **vello 路径的渲染结果不能变**：不设 `FENESTRA_RENDERER` 时，所有现有快照必须原样通过，禁止 `FENESTRA_UPDATE_SNAPSHOTS=1`。
- 依赖版本以本计划写的为准，加依赖后用 `cargo tree -d -p fenestra-raster` 确认 peniko、kurbo、wgpu 都只有一份。
- 颜色管线和 vello 一致：渲染目标是 `Rgba8Unorm`（不用 sRGB 格式），在 sRGB 编码值上做预乘混合 `One, OneMinusSrcAlpha`；渐变表每行 512 个样本，生成方式照搬 vello_encoding 0.9.0 `ramp_cache.rs` 的 `make_ramp`（MIT/Apache，注明出处），取样方式和 vello 一样取最近样本：`index = u32(clamp(t, 0, 1) * 511 + 0.5)`。
- MSAA 固定 4 倍；模板缓冲用 `Stencil8`，和颜色附件同样 4 倍采样。
- `FENESTRA_RENDERER`：取值 `vello` / `raster`，其他值（包括空值）按平台默认处理：`target_os = "android"` 默认 raster，其余平台默认 vello。适配器后端是 GL 时，即使选了 raster 也退回 vello，并且只打印一次日志。
- 门禁：`cargo fmt --all --check`；`cargo clippy -p fenestra-core -p fenestra-shell -p fenestra -p fenestra-raster --all-targets -- -D warnings`；Android 端 clippy 用上一轮的命令（`--target aarch64-linux-android --no-default-features --features android-game-activity`，并用 `--config` 打上 winit fork 补丁），跑完把 `Cargo.lock` 恢复原样。
- 磁盘：只用一个共享的 `CARGO_TARGET_DIR`，跑测试时设 `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0`，禁止 `--workspace`，跑完 `rm -rf $CARGO_TARGET_DIR/debug`。
- 真机：每条 adb 命令都显式设置 `ANDROID_SERIAL`；不改系统设置；测完清掉探针的属性。
- 下文的 `$SCRATCH` 指会话 scratchpad，`SUITE` 指命令 `cargo test -p fenestra-core -p fenestra-shell -p fenestra -p fenestra-kit -p fenestra-looks -p fenestra-raster`。

## Review Focus

1. **同一个渲染器换尺寸**：窗口改变大小、屏幕旋转后，MSAA、模板和离屏纹理都要按新尺寸重建，不能出现 wgpu 校验错误或画面错位。（Task 2 测试 `renders_at_a_new_size_after_a_resize`）
2. **深层嵌套**：滚动视图（裁剪）里的退场 ghost（透明度图层 + 缩放），里面还有圆角裁剪的图片。期望各层的裁剪、透明度和变换都与 vello 一致。（Task 4 测试 `clip_in_layer_in_clip_under_a_transform`）
3. **图集压力**：一帧里出现的不同汉字超过一页图集的容量时，文字不能丢；下一帧多出的页要回收。（Task 6 测试 `a_frame_larger_than_one_page_draws_every_glyph`）
4. **空帧和退化图形**：空帧、零面积矩形、零宽描边、空路径、空图层，都不能触发 wgpu 校验错误（wgpu 拒绝零大小缓冲区）。（Task 2 测试 `empty_and_degenerate_draws_render_nothing`）
5. **超过设备上限的图片**：手机 GPU 的最大纹理边长可能是 4096，这时 8192 宽的图片上传前要先缩小，不能触发校验错误，画面按比例正确。（Task 5 测试 `an_image_larger_than_the_texture_limit_is_downscaled`）

---

### Task 1: core 提供泛型的两遍绘制入口

**Files:**
- Modify: `fenestra-core/src/frame.rs`（`paint_backdrop` 2247、`paint_final` 2263 旁边）
- Test: `fenestra-core/src/draw.rs` tests 模块

**Interfaces:**
- Consumes: 现有的 `paint_walk<T: DrawTarget>(&self, &mut T, &mut Fonts, &mut FrameState, &mut PaintMode<'_>)`。
- Produces:
  ```rust
  impl Frame {
      /// First pass of the backdrop-blur pipeline into any target: every filtered subtree skipped, its spec returned.
      pub fn paint_backdrop_into<T: DrawTarget>(&self, target: &mut T, fonts: &mut Fonts, state: &mut FrameState) -> Vec<MultiPassSpec>;
      /// Second pass into any target, compositing each injected image in place of its node.
      pub fn paint_final_into<T: DrawTarget>(&self, target: &mut T, fonts: &mut Fonts, state: &mut FrameState, injected: &HashMap<WidgetId, peniko::ImageData>);
  }
  ```
  `paint_backdrop` / `paint_final` 改为调用这两个方法再包一层 `SceneTarget`（返回值不变）。

- [ ] **Step 1: 写失败的测试**（`draw.rs`）：`backdrop_into_skips_glass_and_reports_it`：根节点里放一个 `.backdrop_blur(12.0)` 的 `div` 和一个普通 `div`，`paint_backdrop_into(&mut Recorder)` 返回 1 个 `PassKind::BackdropBlur` spec，而且记录里没有玻璃节点的 Fill（比较 Fill 的个数：只有玻璃时为 0）；`final_into_draws_the_injected_image`：同一个 frame，`paint_final_into` 传入 `{玻璃 id: 4×4 图片}`，记录里出现 `Op::Image`。
- [ ] **Step 2: 运行确认失败** — `cargo test -p fenestra-core --lib draw::` → 编译失败（方法未定义）。
- [ ] **Step 3: 实现**两个方法；`paint_backdrop`/`paint_final` 改为调用它们。
- [ ] **Step 4: 运行** — 同一命令 → 全部通过；再跑一遍 SUITE（vello 路径）→ 0 失败。
- [ ] **Step 5: 门禁、提交** `feat(core): paint_backdrop_into / paint_final_into for any DrawTarget`

### Task 2: fenestra-raster 骨架：纯色填充与描边、变换栈、渲染到纹理

**Files:**
- Create: `fenestra-raster/Cargo.toml`、`src/lib.rs`（`RasterRenderer`、`RasterFrame`、`RenderStats`）、`src/geometry.rs`（路径 → 三角形）、`src/shaders.wgsl`、`tests/support/mod.rs`（vello 对照渲染、读回、差分比较）、`tests/solid.rs`
- Modify: 根 `Cargo.toml`（members 加 `fenestra-raster`；workspace.dependencies 加 `fenestra-raster`、`lyon_tessellation`、`etagere`、`vello_cpu`、`skrifa`、`bytemuck`）

**Interfaces:**
- Consumes: `fenestra_core::draw::{DrawTarget, Glyph, GlyphRun}`。
- Produces:
  ```rust
  pub struct RasterRenderer { /* 管线、MSAA/模板纹理（按尺寸缓存）、各类缓存 */ }
  impl RasterRenderer {
      /// Pipelines for targets of `format` (fenestra renders into Rgba8Unorm).
      pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self;
      /// Starts recording a `width`×`height` device-pixel frame.
      pub fn frame(&mut self, width: u32, height: u32) -> RasterFrame<'_>;
  }
  pub struct RasterFrame<'r> { /* 图层树、批次、变换栈 */ }
  impl DrawTarget for RasterFrame<'_> { /* 本任务先实现 fill/stroke/push_transform/pop_transform，其余方法先 todo!() 并注明在第几个任务实现 */ }
  impl RasterFrame<'_> {
      /// Encodes, submits, and resolves into `target` (`width`×`height`, the renderer's format, RENDER_ATTACHMENT), cleared to `base_color`.
      pub fn render(self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, base_color: peniko::Color) -> RenderStats;
  }
  /// What one frame cost, for tests and FRAMESTATS.
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub struct RenderStats { pub draws: u32, pub vertices: u32, pub layers: u32, pub layer_textures_created: u32, pub images_uploaded: u32, pub glyphs_rasterized: u32 }
  /// What the renderer keeps between frames.
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub struct CacheSizes { pub layer_textures: usize, pub images: usize, pub ramp_rows: usize, pub atlas_pages: usize }
  impl RasterRenderer { pub fn cache_sizes(&self) -> CacheSizes; }
  ```
  集成测试（`tests/*.rs`）看不到 `#[cfg(test)]` 的东西，所以缓存、池和图集的行为都通过 `RenderStats` 和 `cache_sizes()` 观察。各字段在实现对应功能的任务里填上，此前保持为 0。
  测试支持：`trait Paint { fn paint<T: DrawTarget>(&self, t: &mut T); }`；`fn assert_like_vello(name: &str, scene: &impl Paint, size: (u32, u32), budget: Budget)`；`struct Budget { channel: u8, max_frac: f64 }`。实现：用 vello `Renderer` 渲染 `SceneTarget` 版本，用 `RasterRenderer` 渲染 raster 版本，两张图都读回；统计任一通道差值大于 `channel` 的像素占比，超过 `max_frac` 就失败，并把 `name.vello.png`、`name.raster.png`、`name.diff.png` 写到 `$CARGO_TARGET_TMPDIR`。GPU 设备用 `OnceLock` 共享。
- 几何：变换在 CPU 上作用到顶点；三角化容差为 `0.25 / sqrt(|det(transform)|)`（下限 0.01）；虚线先用 `kurbo::dash` 展开再描边；`Stroke` 的 width/join/miter_limit/start_cap/end_cap 全部映射到 lyon 的对应选项；`Fill::NonZero/EvenOdd` 映射到 lyon 的 `FillRule`。三角化出错时跳过这一个图形，并用 `log::warn!` 打一次日志，不 panic。

- [ ] **Step 1: 写失败的测试** `tests/solid.rs`（默认 `Budget { channel: 3, max_frac: 0.01 }`）：
  - `rounded_rect_fill_matches_vello`：200×120 画布，圆角 16 的矩形。
  - `strokes_match_vello`：2px 实线圆角框、`with_dashes(0.0, [8.0, 8.0])` 的虚线、圆头圆角折线。
  - `even_odd_fill_matches_vello`：两个同心圆，EvenOdd。
  - `transformed_fill_matches_vello`：`push_transform(rotate(30°)·scale(1.5))` 下的矩形。
  - `renders_at_a_new_size_after_a_resize`（Review Focus 1）：同一个 renderer 先渲染 100×80，再渲染 300×200，两次都和 vello 对得上。
  - `empty_and_degenerate_draws_render_nothing`（Review Focus 4）：空帧、零面积矩形、零宽描边、空 `BezPath`，渲染结果全是 base color，`RenderStats::draws == 0`。
- [ ] **Step 2: 运行确认失败** — `cargo test -p fenestra-raster --test solid` → 编译失败（crate 不存在 / 类型未定义）。
- [ ] **Step 3: 实现** crate 骨架、几何、纯色管线、MSAA 解析到 target、测试支持。原型 `rbench/src/raster.rs` 的着色器和管线可以照搬后再改。
- [ ] **Step 4: 运行** — 同一命令 → 6 个测试通过。
- [ ] **Step 5: 门禁、提交** `feat(raster): fenestra-raster with solid fills and strokes`

### Task 3: 渐变（线性、径向、扫描）

**Files:** Create `fenestra-raster/src/paint.rs`（画刷 → 顶点参数，渐变表）；Modify `src/lib.rs`、`src/shaders.wgsl`；Test `tests/gradient.rs`

**Interfaces:**
- Consumes: Task 2 的几何和管线。
- Produces: 顶点格式 `PaintV { pos: [f32; 2], local: [f32; 2], color: [f32; 4], grad: [f32; 4], grad2: [f32; 4] }`。`local` 是变换前的坐标，渐变在 local 空间里算，任意仿射变换下都正确。`grad2` 打包渐变种类（0 纯色 / 1 线性 / 2 径向 / 3 扫描）、渐变表行号和 Extend。渐变表是 512 宽的 `Rgba8Unorm` 纹理，每种色标列表占一行，同一帧内按色标列表去重；帧数超过 64 行时，丢弃最近一帧没用到的行。fenestra 只会构造 `new_linear`、单圆的 `new_radial`、`new_sweep(c, 0, TAU)`，遇到双圆锥形径向渐变或图片画刷，`debug_assert!` 失败，release 下用第一个色标的纯色绘制。

- [ ] **Step 1: 写失败的测试**（`Budget { channel: 3, max_frac: 0.01 }`）：`linear_gradient_matches_vello`（3 个色标，45°）、`radial_gradient_matches_vello`、`sweep_gradient_matches_vello`、`gradient_under_rotation_matches_vello`（rotate 30° 下的线性渐变）、`translucent_stops_match_vello`（色标带 alpha，验证先插值后预乘）。
- [ ] **Step 2: 运行确认失败** — `cargo test -p fenestra-raster --test gradient` → 失败（渐变画成了纯色或者 todo!）。
- [ ] **Step 3: 实现**渐变表（移植 `make_ramp`）和片元着色器里的三种 t 计算：线性 `dot(p-a, b-a)/|b-a|²`；径向 `|p-c|/r`；扫描 `fract(atan2(p-c)/TAU)`，起始角度和方向要和 vello 的 sweep 约定对齐，实现前先读 vello_shaders 0.9.0 `fine.wgsl` 里对应的分支。
- [ ] **Step 4: 运行** → 5 个测试通过，Task 2 的测试仍然通过。
- [ ] **Step 5: 门禁、提交** `feat(raster): linear, radial, and sweep gradients`

### Task 4: 裁剪图层与透明度图层

**Files:** Create `fenestra-raster/src/layers.rs`（图层树、模板层级、离屏纹理池）；Modify `src/lib.rs`、`src/shaders.wgsl`；Test `tests/layers.rs`

**Interfaces:**
- Produces（`RasterFrame` 内部语义，测试可以观察到的部分）：
  - `push_clip_layer`：在当前 pass 里把裁剪形状写进模板缓冲（compare `Equal(level)`，pass op `IncrementClamp`），之后的绘制用 `Equal(level+1)` 测试；`pop_layer` 用同一组三角形 `DecrementClamp` 还原。层级超过 255 时 `debug_assert!`。
  - `push_layer(alpha)`：开一个子图层，内容画进池里的离屏 `Rgba8Unorm` 纹理（MSAA 解析后），纹理大小 = 裁剪形状的设备包围盒 ∩ 画布，向外取整；`pop_layer` 时在父图层里先把裁剪形状写进模板，再画一个 `alpha × 纹理` 的合成四边形。alpha ≥ 1 的 `push_layer` 按 clip layer 处理（不开离屏纹理）。
  - `render()` 按后序画：子图层先画，父图层的 pass 再合成。离屏纹理用完放回池里，按尺寸复用；连续 60 帧没用到的纹理释放。
- [ ] **Step 1: 写失败的测试**（`Budget { channel: 3, max_frac: 0.02 }`；MSAA 和解析 AA 在裁剪边缘有差异，所以放宽到 2%）：
  - `rounded_clip_matches_vello`
  - `nested_clips_match_vello`（矩形 ∩ 圆）
  - `opacity_layer_composites_children_once`（两个重叠的半透明子矩形放在 alpha 0.5 的图层里：重叠处不能比单个子矩形更深，和 vello 一致）
  - `clip_in_layer_in_clip_under_a_transform`（Review Focus 2：滚动裁剪 → alpha 0.6 图层 → `push_transform(scale 0.8 about center)` → 圆角裁剪 → 图片之外的纯色内容）
  - `stencil_restores_after_pop`（pop 之后在裁剪区域外画的内容可见）
  - `layer_textures_are_pooled`（连续渲染两帧同样的场景，第二帧 `RenderStats::layers` 相同，第二帧的 `layer_textures_created == 0`）
- [ ] **Step 2: 运行确认失败** — `cargo test -p fenestra-raster --test layers` → 失败（todo! panic）。
- [ ] **Step 3: 实现**图层树、模板管线变体（每条管线一个 stencil 版本）、合成管线和纹理池。
- [ ] **Step 4: 运行** → 6 个测试通过；Task 2–3 的测试仍然通过。
- [ ] **Step 5: 门禁、提交** `feat(raster): stencil clips and offscreen opacity layers`

### Task 5: 图片与模糊圆角矩形（阴影）

**Files:** Create `fenestra-raster/src/images.rs`；Modify `src/lib.rs`、`src/shaders.wgsl`；Test `tests/images.rs`

**Interfaces:**
- 图片：按 `(image.data.id(), width, height, format, alpha_type)` 缓存纹理，上传时统一转成预乘 RGBA；连续 120 帧没用到就释放。超过 `device.limits().max_texture_dimension_2d` 时先在 CPU 上按比例缩小（box filter），绘制时 uv 不变。绘制是一个带完整仿射变换的纹理四边形，用双线性采样。
- 阴影：local 空间四边形 = rect 向外扩 `3·std_dev`，片元着色器用原型里 Evan Wallace 的解析公式（局部坐标，所以任意变换都对）。`std_dev ≤ 0` 时退化为圆角矩形填充。
- [ ] **Step 1: 写失败的测试**：`image_matches_vello`（64×48 渐变图案图片缩放到 200×150，`Budget { channel: 4, max_frac: 0.02 }`）、`bgra_and_unpremultiplied_images_match_vello`、`rotated_image_matches_vello`、`shadow_matches_vello`（std_dev 8、radius 12，`Budget { channel: 4, max_frac: 0.02 }`）、`rotated_shadow_matches_vello`、`an_image_larger_than_the_texture_limit_is_downscaled`（Review Focus 5：用 `#[doc(hidden)] pub fn RasterRenderer::with_max_texture_dimension(device, format, 256)` 模拟上限（集成测试要能调到，所以是 pub），512×256 的图片绘制结果与 vello 一致，`Budget { channel: 8, max_frac: 0.05 }`）、`image_cache_reuses_and_evicts`（同一张图片画两帧，第二帧 `images_uploaded == 0`；之后 121 帧不画它，`cache_sizes().images == 0`）。
- [ ] **Step 2: 运行确认失败** — `cargo test -p fenestra-raster --test images` → 失败。
- [ ] **Step 3: 实现**。
- [ ] **Step 4: 运行** → 7 个测试通过，之前的测试仍然通过。
- [ ] **Step 5: 门禁、提交** `feat(raster): images and analytic blurred rounded rects`

### Task 6: 字形图集

**Files:** Create `fenestra-raster/src/glyphs.rs`；Modify `src/lib.rs`、`src/shaders.wgsl`；Test `tests/text.rs`（拉丁文用 fenestra-core 内置的 Inter；中文和 emoji 字体通过 fontique 系统字体集合按脚本回退去找（`Collection::fallback_families`，Han 和 Emoji），找不到就打印跳过信息直接返回，这样在没有这些字体的 CI 上也不会失败）

**Interfaces:**
- 图集：两种页面，单色 `R8Unorm` 1024²，彩色 `Rgba8Unorm` 1024²（预乘），每页一个 `etagere::AtlasAllocator`，字形四周留 1px 间距。
- 缓存键：`(font.data.id(), font.index, glyph id, size_q, coords 的哈希, hint, glyph_transform 的位模式, x_bin, y_bin)`。`size_q` = `font_size × s`，其中 `s = sqrt(|det(当前变换·run.transform)|)`，量化到 1/16px；x/y 各分 4 个亚像素档。
- 光栅化：按原型 `cell()` 的方式，用 `vello_cpu::RenderContext` + `Resources` 在小 pixmap 上画单个字形，再加上 hint、normalized_coords 和 glyph_transform。这几个 builder 方法以 vello_cpu 0.2.0 源码为准，不要凭记忆写。单元格尺寸来自 skrifa `glyph_metrics(..).bounds(id)`；没有 bounds（位图字形）时用 `1.3·size × 1.45·size`。用 skrifa 的 `color_glyphs()` 或 `bitmap_strikes()` 判断是不是彩色字形：彩色字形用白色画刷画进彩色页，绘制时颜色 = 纹理 × run.color 的 alpha；单色字形取 pixmap 的 alpha 存进 R8 页，绘制时颜色 = run.color × 覆盖率。
- 绘制：每个字形一个四边形。四边形先按 `s` 缩放后的坐标摆放，再乘上去掉缩放后的剩余变换（旋转、斜切），用双线性采样。
- 淘汰：分配失败时，先淘汰同类页里本帧没用到、最久没用的字形；还放不下就加一页；`render()` 结束时，超过 4 页且本帧没用到的页释放掉。字形永远不丢。
- [ ] **Step 1: 写失败的测试**：
  - `latin_text_matches_vello`（Inter 16px 两行，`Budget { channel: 24, max_frac: 0.03 }`：两种光栅化在文字边缘的 AA 必然有差异，这个阈值用来抓错位、漏字和字号错，不追求逐像素一致）
  - `cjk_text_matches_vello`（PingFang 中文一行）
  - `emoji_is_drawn_in_color`（👍 区域里有饱和度 > 0.3 的像素）
  - `synthetic_italic_is_skewed`（glyph_transform 有 skew 时，和 vello 对比在预算内，并且和不斜切的版本不同）
  - `scaled_text_is_rasterized_at_the_scaled_size`（`push_transform(scale 2)` 下的 16px 文本，和 vello 的 32px 渲染对比）
  - `a_second_frame_rasterizes_nothing`（同样的文本渲染两帧，第二帧 `RenderStats::glyphs_rasterized == 0`）
  - `a_frame_larger_than_one_page_draws_every_glyph`（Review Focus 3：一帧画 3000 个不同汉字，每个 48px，都能找到非背景像素；下一帧只画 10 个字后，`cache_sizes().atlas_pages ≤ 4`）
- [ ] **Step 2: 运行确认失败** — `cargo test -p fenestra-raster --test text` → 失败。
- [ ] **Step 3: 实现**。
- [ ] **Step 4: 运行** → 7 个测试通过，之前的测试仍然通过。
- [ ] **Step 5: 门禁、提交** `feat(raster): glyph atlas with color glyphs, subpixel bins, and eviction`

### Task 7: shell 选后端；无头渲染与快照走光栅化

**Files:**
- Create: `fenestra-shell/src/backend.rs`
- Modify: `fenestra-shell/src/headless.rs`、`fenestra-shell/src/lib.rs`、`fenestra-shell/Cargo.toml`（依赖 `fenestra-raster`）
- Test: `backend.rs` 单元测试；`fenestra-shell/tests/raster_headless.rs`

**Interfaces:**
- Consumes: Task 1 的 `paint_backdrop_into` / `paint_final_into`；Task 2 的 `RasterRenderer`。
- Produces:
  ```rust
  /// Which rasterizer draws frames.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum Backend { Vello, Raster }
  /// Env var: `vello` or `raster`; anything else means the platform default.
  pub const RENDERER_ENV: &str = "FENESTRA_RENDERER";
  pub fn backend_from_env() -> Backend;
  pub(crate) fn parse_backend(v: Option<&std::ffi::OsStr>, platform_default: Backend) -> Backend;
  /// Raster needs Vulkan, Metal, or DX12; on GL the shell keeps vello.
  pub(crate) fn raster_supported(backend: wgpu::Backend) -> bool;
  impl Headless { pub fn backend(&self) -> Backend; pub fn with_backend(backend: Backend) -> Result<Self, ShellError>; }
  ```
  `Headless::new()` = `with_backend(backend_from_env())`。raster 模式下 `render_plan` 不经过 `Scene`：先 `frame.paint_backdrop_into`，在 `RasterFrame` 上 `push_transform(translate(-region.x, -region.y)·scale(scale))`，区域用 `multi_pass::backdrop_region` 算；读回后交给 `process_specs_in`；最后 `paint_final_into` 画整帧。读回逻辑从 `Headless::render` 里抽成 `fn read_texture(device, queue, texture, w, h) -> Result<RgbaImage, ShellError>`，两条路径共用。`Headless::render(&Scene)` 仍然只走 vello（vello 的 `Renderer` 改为第一次用到时才创建）。
- [ ] **Step 1: 写失败的测试**：`parse_backend` 的取值表（None、`""`、`"vello"`、`"raster"`、`"RASTER"`、`"x"` → 默认值）；`raster_supported` 对 Gl 返回 false，对 Vulkan/Metal/Dx12 返回 true；`tests/raster_headless.rs`：`glass_renders_through_raster`（一个带玻璃卡片的视图，用 `Backend::Raster` 和 `Backend::Vello` 各渲染一次，差值在 `Budget { channel: 24, max_frac: 0.03 }` 以内，并且玻璃区域确实被模糊了：区域内的高频能量低于未模糊的背景）；`hidpi_raster_matches_vello`（scale 2）。
- [ ] **Step 2: 运行确认失败** — `cargo test -p fenestra-shell --lib backend` 和 `cargo test -p fenestra-shell --test raster_headless` → 失败。
- [ ] **Step 3: 实现**。
- [ ] **Step 4: 运行** → 通过；SUITE（默认 vello）0 失败，并且快照文件没有任何变动（`git status` 干净）。
- [ ] **Step 5: 用光栅化后端整体跑一遍快照**：`FENESTRA_RENDERER=raster FENESTRA_SNAPSHOT_BUDGET=0.02 cargo test -p fenestra-kit -p fenestra-looks -p fenestra-charts -p fenestra-markdown -p fenestra-motion -p fenestra-a2ui -p fenestra-describe -p fenestra > $SCRATCH/raster-goldens.log 2>&1`。Expected：不在快照比对里的测试全部通过；快照失败的数量和名单记进账本，每个失败的 `.side.png` 都打开看一遍。错位、缺内容、裁剪错、颜色错属于 bug，回到对应任务修（修的时候先写复现测试）；纯 AA 差异不算 bug，记一条 `Ruling:`。这一步的完成标准：没有一个失败是 bug。2% 是硬上限，不允许为了让快照通过而放宽。
- [ ] **Step 6: 门禁、提交** `feat(shell): FENESTRA_RENDERER selects vello or the raster backend; headless renders through either`

### Task 8: 窗口接入、Android 默认光栅化、真机验证

**Files:**
- Modify: `fenestra-shell/src/window.rs`（`WindowShell` 的 activate/resize/present/render_to_image；`AppRunner` 主窗口重绘；FRAMESTATS 的 Render 阶段）
- Modify（探针，不在仓库里）：`scratchpad/fenprobe/src/lib.rs` 支持 `debug.fenprobe.renderer`（设置 `FENESTRA_RENDERER`）
- Test: `window.rs` 里的纯函数单元测试；真机

**Interfaces:**
- Consumes: Task 7 的 `Backend`、`raster_supported`；Task 1、2。
- Produces:
  - `WindowShell` 增加 `raster: Option<RasterState>`，其中 `struct RasterState { renderer: RasterRenderer, target: wgpu::Texture, view: wgpu::TextureView }`。target 是 `Rgba8Unorm`，usage 为 `RENDER_ATTACHMENT | TEXTURE_BINDING | COPY_SRC`，随 `resized` 重建。
  - `fn present_raster(&mut self, paint: impl FnOnce(&mut RasterFrame<'_>)) -> Result<(), ShellError>`：渲染到 target，再用 surface 现有的 `blitter` 拷到交换链纹理，Acquire/Present 计时和 vello 路径相同。
  - `fn render_raster_to_image(&mut self, size: (u32, u32), paint: impl FnOnce(&mut RasterFrame<'_>), bg: Color) -> Option<Result<RgbaImage, ShellError>>`：给两遍渲染读回用。
  - raster 模式下不使用 `cached_scene`：帧不脏时只重新 blit 上一帧的 target。
  - `fn effective_backend(requested: Backend, adapter: wgpu::Backend) -> Backend`（纯函数，GL 上退回 vello）。
- [ ] **Step 1: 写失败的测试**：`effective_backend` 的四种组合；`raster_target_tracks_the_surface_size`：把"按 surface 尺寸决定是否重建"抽成纯函数 `needs_new_target(current: Option<(u32, u32)>, surface: (u32, u32)) -> bool` 并测试它。
- [ ] **Step 2: 运行确认失败** → 编译失败。
- [ ] **Step 3: 实现**窗口路径（单遍和两遍都要实现）。
- [ ] **Step 4: 运行** → 通过；SUITE 0 失败；门禁（含 Android clippy）通过。
- [ ] **Step 5: 真机**（一加 6 `d0305cb2`，华为 `TNL0220A13019087` 有连接时也测）：重新打包探针。场景用 `static`、`vlist`、`images`、`ime`，以及一个带玻璃的场景（探针里没有就新增 `glass` 场景：滚动的图片列表上盖一条毛玻璃顶栏）。每个场景分别用 `renderer=raster` 和 `renderer=vello cpu=1` 截图，用上一轮的 `compare.py`/`align.py` 比较，`stats=1` 记录 FRAMESTATS。Expected：
  - 两种后端的截图肉眼看不出差别，像素统计记进报告。
  - raster 的 render 阶段 p50 比 vello CPU 路径低一个数量级（一加上 vello CPU 的 `static` render 约 85ms）。
  - `ime` 场景用 Gboard 拼音输入"你好"能上屏。
  - 没有 wgpu 校验错误：`adb logcat | grep -i "wgpu\|validation"` 为空。
- [ ] **Step 6: 报告**：在笔记目录的 `BACKEND-REPORT.md` 新增"光栅化后端已接入"一节，写清提交号、Task 7 的快照统计和裁决、两台手机上的帧时间表，以及留给第三份计划的问题（三角化缓存、缓冲区复用、GPU 玻璃、GLES）。
- [ ] **Step 7: 门禁、清理、提交** `feat(shell): the window renders through the raster backend; Android defaults to it`
