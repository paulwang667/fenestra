# 光栅化后端 GPU 毛玻璃 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 光栅化后端里的毛玻璃（模糊、饱和度、圆角折射、底色与高光合成、自适应底色）全部在 GPU 上完成，一帧里不读回；然后在 wechat-ui 上试一次大面积玻璃。

**Architecture:** fenestra-core 新增两个只增不改的接口：`Frame::glass_body(id)` 给后端玻璃本体参数，`Frame::paint_final_baked_into(…, baked)` 让最终绘制跳过已在 GPU 合成的玻璃。fenestra-raster 新增 `glass` 模块：从背景帧纹理出发，用普通渲染管线（全屏三角形加片元着色器）依次做降采样转线性、三遍方框模糊、饱和度和上采样、折射、平均亮度和自适应底色、本体合成加抖动，结果纹理用占位 `ImageData` 登记进图片缓存。fenestra-shell 的两条光栅化路径（无头 `render_plan_raster`、窗口 `present_raster_frame`）把 `BackdropBlur` 交给 GPU，其余（`ElementFilter`、`Custom`）仍走读回。

**Tech Stack:** Rust 2024，wgpu 29（渲染管线，无 compute），WGSL，fenestra-core / fenestra-raster / fenestra-shell；wechat-ui（试验）。

**Spec:** `/Users/xxxx/projects/fenestra/docs/superpowers/specs/2026-10-27-gpu-glass-blur-design.md`

## Global Constraints

- 仓库：fenestra `/Users/xxxx/projects/fenestra`，在 `main` 上提交，推到 `fork`（`paulwang667/fenestra`）的 `main`，推送后手动 `gh workflow run CI --repo paulwang667/fenestra --ref main`。wechat-ui `/Users/xxxx/projects/wechat-ui` 只本地提交。
- 构建目录：`export CARGO_TARGET_DIR=/private/tmp/claude-501/-Users-xxxx-projects-wechat-ui/06fb374e-ccba-4302-b42c-3e65fef847d8/scratchpad/target CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0`；**不跑 `cargo test --workspace`**，按包跑。wechat-ui 不跑 `cargo fmt`（仓库没格式化过）。
- 整个 fenestra 工作区禁止 `unsafe`。
- 只改光栅化后端；vello 后端的输出和桌面行为不变；`paint_final_into`、`MultiPassSpec`、`PassKind` 等已有公开接口的签名和行为不变（fenestra-core 只新增）。
- wasm32 不支持 GPU 玻璃（保持现有不模糊分支），但必须能编译；Android、iOS 也必须能编译。
- 玻璃的数值规则照搬 CPU 版：
  - 方框半径 `r = box_radius_for_std_dev(std_dev)`；读取边距 `blur_reach(std_dev)`（= 3r）。
  - 降采样系数 `downsample_factor(r)`：2 的幂，最大 `MAX_DOWNSAMPLE = 4`，降采样后半径不小于 `MIN_LOW_RADIUS = 4`。
  - `f > 1` 时三遍半径 `three_box_radii(r(r+1)/f² − ¼)`；`f == 1` 时三遍都是 `r`。
  - 饱和度：线性空间按 Rec.709 亮度 `l + s(c − l)`，下限 0。
  - 折射：边缘带宽 `EDGE = 7.0 × scale`；最大位移 `0.55 × band`；位移 `max_disp·(1−d/band)²`；色散 `(0.94, 1.06)`；圆角取四角平均；只在玻璃完整在画面内时做。
  - 本体：在 sRGB 里源覆盖合成底色，再合成高光（`SheenLine` 的几何和三个停点，0.55 处转换）。
  - 自适应底色：sRGB 上 Rec.709 亮度、步长 4 取样求平均，`L' = clamp(L + gain·(pivot − luma))`，在 OKLCH 里平移后按 `theme::oklch` 的规则映射回 sRGB（超色域时对色度二分 24 次）。
  - 抖动：`dither_noise` 的哈希原样移植到 WGSL。
- 中间纹理用 `Rgba16Float`（线性空间和未量化的 sRGB 值），结果纹理用 `Rgba8Unorm`、预乘（图片缓存的约定）。只在最后一步量化一次。
- 一加 6 序列号 `d0305cb2`，adb 时设 `ANDROID_SERIAL`；测完清掉 `debug.wechatui.*` 属性；默认输入法保持 Gboard。

## Review Focus

1. 玻璃滚到一半出了画面（比如被顶栏挡住的列表区域、滚动中的通话卡片）：内容不被拉伸、不折射、边缘没有接缝。→ Task 6 `a_pane_partly_off_screen_is_not_stretched`
2. 两块玻璃叠在一起（菜单盖在通话卡片上）：每块都模糊"不含任何玻璃"的背景，和 CPU 路径一致，不互相看见。→ Task 6 `overlapping_panes_each_blur_the_glassless_backdrop`
3. 极小或退化的玻璃（宽或高为 0、小于 4 像素、`std_dev` 为 0）：不崩溃、不出错误纹理；`std_dev` 为 0 时就是背景原样加本体。→ Task 3 `tiny_and_unblurred_panes_do_not_fail`
4. 同一块玻璃尺寸变化（旋转、窗口缩放、动画中的卡片）：纹理按新尺寸重建，不复用旧尺寸；玻璃消失后纹理被回收，缓存不无限增长。→ Task 3 `a_pane_that_resizes_or_goes_away_keeps_no_stale_texture`
5. 一帧里玻璃和元素滤镜同时出现：玻璃走 GPU，只为元素滤镜读回一次；没有玻璃的帧不受影响。→ Task 6 `glass_and_an_element_filter_read_back_only_the_filter`

---

### Task 1: fenestra-core 的玻璃本体接口

**Files:**
- Modify: `fenestra-core/src/paint_plan.rs`（`PaintMode::Final` 带上已合成集合；新类型 `GlassBody`）
- Modify: `fenestra-core/src/frame.rs`（`Frame::glass_body`、`Frame::paint_final_baked_into`；`paint_node` 里玻璃分支对已合成的跳过 `adapt_fill` 和 `bake_cached`）
- Modify: `fenestra-core/src/lib.rs`（导出 `GlassBody`）
- Test: `fenestra-core/src/frame.rs` 测试模块

**Interfaces:**
- Produces:
  ```rust
  /// What a backend needs to composite a glass pane's body itself.
  #[derive(Debug, Clone, Copy, PartialEq)]
  pub struct GlassBody {
      /// The pane's logical rect (the sheen's geometry is laid over it).
      pub rect: kurbo::Rect,
      /// The solid fill (`None` when the fill is not solid or absent).
      pub tint: Option<peniko::Color>,
      pub sheen: Option<crate::style::Sheen>,
      pub adaptive: Option<crate::style::AdaptiveTint>,
  }
  impl Frame {
      /// The body of the glass pane `id`, or `None` when it is not glass.
      pub fn glass_body(&self, id: WidgetId) -> Option<GlassBody>;
      /// `paint_final_into`, except that the panes in `baked` already have
      /// their body (tint, sheen, adaptive tint) in their injected image: the
      /// image is drawn as it is, and the box loses the fill, sheen and
      /// adaptive tint, as for a CPU-baked pane.
      pub fn paint_final_baked_into<T: DrawTarget>(
          &self, target: &mut T, fonts: &mut Fonts, state: &mut FrameState,
          injected: &HashMap<WidgetId, peniko::ImageData>,
          baked: &std::collections::HashSet<WidgetId>,
      );
  }
  ```
  （`Sheen`、`AdaptiveTint` 已派生 `Copy`/`PartialEq`。）`paint_final_into` 等价于 `baked` 为空。

- [ ] **Step 1: 写失败的测试**（用一个记录 `draw_image` 的测试 `DrawTarget`：记录每次画的图片的 `data.id()`，以及 `fill` 调用次数和颜色；若测试模块里已有类似的记录器，复用它）

```rust
#[test]
fn glass_body_reports_the_pane_and_nothing_else() {
    // A glass pane (backdrop_blur, solid tint, sheen, adaptive tint) next to a plain box.
    // glass_body(pane) == Some(GlassBody { rect, tint: Some(tint), sheen: Some(s), adaptive: Some(a) })
    // glass_body(plain) == None
}

#[test]
fn a_baked_pane_draws_its_image_as_it_is() {
    // Inject a 1×1-blob image for the pane; paint_final_baked_into with baked = {pane}:
    // the recorded draw_image ids contain exactly the injected blob id (no CPU-baked copy),
    // and no fill in the tint's colour is recorded.
    // The same frame through paint_final_into: the drawn image id is NOT the injected one (CPU bake).
}
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test -p fenestra-core --lib glass_body a_baked_pane`；Expected: 编译失败（`GlassBody`、`glass_body`、`paint_final_baked_into` 不存在）。
- [ ] **Step 3: 实现**。`glass_body` 用已有的 `frame_node(id)` 找节点，只对 `style.backdrop_blur.is_some()` 返回。`PaintMode::Final(&map, Option<&HashSet>)`（crate 内部枚举，可改）；`paint_node` 的玻璃分支：在已合成集合里时，跳过 `adapt_fill` 和 `bake_cached`，直接以注入图片作为 `backdrop` 传给 `push_box`，样式按现在 baked 分支的规则去掉 `fill`（仅当原 fill 为纯色）、`sheen`、`adaptive_tint`。
- [ ] **Step 4: 跑测试确认通过** — `cargo test -p fenestra-core --lib`；Expected: 全部 PASS。
- [ ] **Step 5: 提交** — `git commit -m "feat(core): glass_body and paint_final_baked_into for backends that bake glass themselves"`

### Task 2: 光栅化图片缓存登记外部纹理

**Files:**
- Modify: `fenestra-raster/src/images.rs`（登记外部纹理的条目）
- Modify: `fenestra-raster/src/lib.rs`（`RasterRenderer::register_texture`；新增 `pub mod`/导出留给 Task 3）
- Test: `fenestra-raster/tests/images.rs`

**Interfaces:**
- Produces:
  ```rust
  impl RasterRenderer {
      /// Makes drawing `placeholder` draw `view` instead (premultiplied
      /// `Rgba8Unorm`, the placeholder's width × height). The placeholder's
      /// pixels are never read or uploaded. Kept like any cached image: dropped
      /// after it goes undrawn for the cache's frame count.
      pub fn register_texture(&mut self, placeholder: &peniko::ImageData, view: wgpu::TextureView);
  }
  /// A placeholder image for a texture: a unique blob of one byte, the given size.
  pub fn texture_placeholder(width: u32, height: u32) -> peniko::ImageData;
  ```

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn a_registered_texture_draws_in_place_of_its_placeholder() {
    // Make a 4×4 premultiplied texture of solid (200, 40, 60, 255) via queue.write_texture.
    // let ph = texture_placeholder(4, 4); r.register_texture(&ph, view);
    // A frame that draws `ph` scaled into Rect(10,10,50,50): the rendered pixel at (30,30)
    // is (200, 40, 60) within 2/255, and RenderStats::images_uploaded == 0.
}
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test -p fenestra-raster --test images registered_texture`；Expected: 编译失败。
- [ ] **Step 3: 实现**：`ImageCache` 的条目加一个"外部"标志或单独表；`use_image` 对已登记的键直接命中；`upload` 永远不处理它；`begin_frame` 的过期规则一样适用。
- [ ] **Step 4: 跑测试确认通过** — `cargo test -p fenestra-raster`；Expected: 全部 PASS。
- [ ] **Step 5: 提交** — `git commit -m "feat(raster): a placeholder image can stand for a GPU texture"`

### Task 3: GPU 模糊（降采样、三遍方框、饱和度、上采样）

**Files:**
- Create: `fenestra-raster/src/glass.rs`、`fenestra-raster/src/glass.wgsl`
- Modify: `fenestra-raster/src/lib.rs`（`mod glass; pub use glass::{GlassPane, GlassStats};`、`RasterRenderer::glass`）
- Modify: `fenestra-shell/src/blur.rs`、`fenestra-shell/src/multi_pass.rs`（把 `blur.rs` 的 `downsample_factor`、`three_box_radii`、`box_radius_for_std_dev` 和 `multi_pass.rs` 的 `blur_reach` 挪到 fenestra-raster 的 `glass` 模块并 `pub` 导出，shell 改为调用它们，避免两份实现；CPU 行为不变）
- Test: `fenestra-shell/src/multi_pass.rs` 的新测试模块 `gpu_glass_tests`（shell 同时有 CPU 参照和 GPU 设备）

**Interfaces:**
- Consumes: Task 2 的 `register_texture`、`texture_placeholder`；Task 1 的 `GlassBody`。
- Produces:
  ```rust
  /// One glass pane for [`RasterRenderer::glass`], in device pixels.
  #[derive(Debug, Clone, Copy)]
  pub struct GlassPane {
      pub id: fenestra_core::WidgetId,
      /// The pane's rect in device px; may reach past the source (samples clamp).
      pub rect: (i32, i32, u32, u32),
      /// Gaussian standard deviation, device px.
      pub std_dev: f32,
      pub saturation: f32,
      /// Corner radius and edge band, device px; `lensed` only when the pane
      /// is wholly inside the source.
      pub corner_radius: f32,
      pub edge_band: f32,
      pub lensed: bool,
      /// The body to composite; `None` composites nothing over the backdrop.
      pub body: Option<fenestra_core::GlassBody>,
      /// Logical → device scale (to lay the sheen's geometry).
      pub scale: f64,
  }
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
  pub struct GlassStats { pub panes: u32, pub passes: u32, pub textures_created: u32 }
  impl RasterRenderer {
      /// Composites every pane from `source` (the backdrop frame, `Rgba8Unorm`,
      /// TEXTURE_BINDING) on the GPU and submits the work; returns each pane's
      /// placeholder (already registered) and the stats. No readback.
      pub fn glass(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
          source: &wgpu::Texture, panes: &[GlassPane])
          -> (std::collections::HashMap<fenestra_core::WidgetId, peniko::ImageData>, GlassStats);
      /// Test support: reads a registered texture back (premultiplied RGBA8).
      #[doc(hidden)]
      pub fn read_registered(&self, device: &wgpu::Device, queue: &wgpu::Queue,
          placeholder: &peniko::ImageData) -> Option<Vec<u8>>;
  }
  // Pure helpers, moved from fenestra-shell::blur (same behaviour):
  pub fn box_radius_for_std_dev(sigma: f32) -> u32;
  pub fn blur_reach(std_dev: f32) -> u32;
  pub fn downsample_factor(radius: u32) -> u32;
  pub fn three_box_radii(var: f32) -> [u32; 3];
  ```
- GPU 流程（本任务只做到上采样；折射、本体在 Task 4、5 接上，未接时输出就是模糊结果）：
  1. **取样转线性**：从 `source` 读 `rect` 外扩 `blur_reach` 的区域（超出源纹理的坐标按边缘取值），每个低分辨率像素对 `f×f` 块求 sRGB→线性的平均，写入 `Rgba16Float` 低分辨率纹理。
  2. **三遍方框**：每遍先横后竖，半径见 Global Constraints，边缘取值，`Rgba16Float` 两张纹理来回。
  3. **饱和度和上采样**：对玻璃窗口里每个设备像素，按 `(c + 0.5)/f − 0.5` 双线性采样低分辨率场，做饱和度，转回 sRGB（不量化），写入玻璃尺寸的 `Rgba16Float`。
  4. **量化**：（Task 5 前的临时末步）抖动后写入 `Rgba8Unorm` 结果纹理并登记。
- 纹理按 `WidgetId` 复用，尺寸变就重建；本帧没出现的玻璃，其纹理在若干帧后释放（沿用 `LayerPool` 的 `KEEP_FRAMES` 思路）。

- [ ] **Step 1: 写失败的测试**（在 `gpu_glass_tests` 里：造一张有清晰图案的背景纹理，例如彩色竖条加棋盘；`fn cpu(...)` 用 `blur_linear_crop` 生成参照，`fn gpu(...)` 调 `glass` 再 `read_registered`；`fn close(a, b)`：差值超过 8/255 的像素不超过 1%，平均绝对差不超过 1.5/255）

```rust
#[test]
fn a_small_blur_matches_the_cpu_at_full_resolution() { /* std_dev 3.0 → f == 1; saturation 1.0; lensed false; body None */ }
#[test]
fn a_large_blur_matches_the_cpu_downsampled() { /* std_dev 40.0 → f == 4 */ }
#[test]
fn saturation_matches_the_cpu() { /* std_dev 20.0, saturation 1.8 */ }
#[test]
fn tiny_and_unblurred_panes_do_not_fail() {
    // rect w or h == 0 → no entry; 3×3 pane → an entry of 3×3; std_dev 0 → the backdrop as it is (close()).
}
#[test]
fn a_pane_that_resizes_or_goes_away_keeps_no_stale_texture() {
    // Same id at 40×30 then 60×20: second result is 60×20 and close() to the CPU.
    // After many frames without the pane: the renderer holds no texture for it (via GlassStats/cache size).
}
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test -p fenestra-shell --lib gpu_glass_tests`；Expected: 编译失败（`glass` 不存在）。
- [ ] **Step 3: 实现** 纯函数迁移、`glass.rs`、`glass.wgsl`（全屏三角形加各遍片元着色器，参数用 uniform）。
- [ ] **Step 4: 跑测试确认通过** — `cargo test -p fenestra-shell --lib gpu_glass_tests && cargo test -p fenestra-shell --lib blur && cargo test -p fenestra-raster`；Expected: 全部 PASS（CPU 模糊的原有测试也不变）。
- [ ] **Step 5: 提交** — `git commit -m "feat(raster): glass blur on the GPU (downsample, three boxes, vibrancy)"`

### Task 4: GPU 圆角折射

**Files:**
- Modify: `fenestra-raster/src/glass.rs`、`glass.wgsl`
- Test: `fenestra-shell/src/multi_pass.rs` 的 `gpu_glass_tests`

**Interfaces:**
- Consumes: Task 3 的流程。折射作用在第 3 步的 `Rgba16Float` 结果上，输出另一张 `Rgba16Float`；`lensed == false` 时跳过。规则见 Global Constraints（SDF、法线、带宽、位移、色散、只算边缘带）。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn refraction_matches_the_cpu() {
    // A 200×120 pane, corner radius 24 device px, edge band 7×2, lensed true, std_dev 12:
    // reference = refract_edges(&blur_linear_crop(...), 24.0, 14.0); close() over the pane,
    // and close() over just the 14 px band (the part that moves).
}
#[test]
fn an_unlensed_pane_is_not_refracted() {
    // Same pane, lensed false: close() to the plain blur, and NOT close() to the refracted reference in the band.
}
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test -p fenestra-shell --lib refraction_matches_the_cpu`；Expected: FAIL（边缘带差异超出容差）。
- [ ] **Step 3: 实现** 折射遍。
- [ ] **Step 4: 跑测试确认通过** — `cargo test -p fenestra-shell --lib gpu_glass_tests`；Expected: 全部 PASS。
- [ ] **Step 5: 提交** — `git commit -m "feat(raster): glass edge refraction on the GPU"`

### Task 5: GPU 本体合成和自适应底色

**Files:**
- Modify: `fenestra-raster/src/glass.rs`、`glass.wgsl`（新增：平均亮度遍、底色遍、本体合成遍；OKLab/OKLCH 转换和色域二分移植到 WGSL）
- Test: `fenestra-shell/src/multi_pass.rs` 的 `gpu_glass_tests`

**Interfaces:**
- Consumes: Task 1 的 `GlassBody`；Task 4 的折射结果。
- 流程：
  1. **平均亮度**：对折射后的玻璃图（sRGB 值）按步长 4 取样求 Rec.709 亮度的平均，两级缩减到 1×1（第一级写一张最多 16×16 的纹理，第二级 1×1），全在 GPU 上。
  2. **底色**（有 `adaptive` 且 `tint` 存在时）：一个 1×1 的遍，按 `adapt_fill` 的规则算出调整后的底色写入 1×1 纹理；没有 `adaptive` 时直接写原底色。
  3. **本体合成**：每像素：sRGB 背景 → 源覆盖底色（用 1×1 纹理里的颜色和原 alpha）→ 高光（`SheenLine` 的 `origin`/`step` 由 CPU 按 `GlassBody.rect`、玻璃像素尺寸算好放进 uniform）→ `dither_noise` 抖动量化 → 预乘后写入 `Rgba8Unorm` 结果纹理。取代 Task 3 的临时量化步。

- [ ] **Step 1: 写失败的测试**（参照 = CPU 路径：`adapt_fill` 的规则算底色，`glass::bake_body` 合成；若这两个是 crate 私有，在 fenestra-core 加 `#[doc(hidden)] pub` 的测试入口 `glass::bake_body_for_test` / `adapt_tint_for_test`，不改行为）

```rust
#[test]
fn the_body_matches_the_cpu_bake() { /* tint oklch(0.35, 0.01, 250)@0.72, sheen {180°, 0.12, 0.06}, no adaptive; close() */ }
#[test]
fn adaptive_tint_matches_the_cpu_over_dark_and_light() {
    // Backdrop all near-black, then all near-white; AdaptiveTint::glass(); close() each,
    // and the two results' mean lightness differ in the direction adapt_fill moves them.
}
#[test]
fn a_tint_out_of_gamut_after_the_shift_is_mapped_like_the_cpu() {
    // A saturated tint (oklch(0.6, 0.25, 30)) shifted by a dark backdrop: close() to the CPU.
}
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test -p fenestra-shell --lib the_body_matches`；Expected: FAIL。
- [ ] **Step 3: 实现** 三个遍和 WGSL 颜色转换。
- [ ] **Step 4: 跑测试确认通过** — `cargo test -p fenestra-shell --lib gpu_glass_tests && cargo test -p fenestra-core --lib`；Expected: 全部 PASS。
- [ ] **Step 5: 提交** — `git commit -m "feat(raster): the glass body and its adaptive tint on the GPU"`

### Task 6: 接进无头和窗口路径

**Files:**
- Modify: `fenestra-shell/src/multi_pass.rs`（`pub(crate) fn glass_panes(specs: &[MultiPassSpec], frame: &Frame, scale: f64, size: (u32, u32)) -> (Vec<GlassPane>, Vec<MultiPassSpec>)`：把 `BackdropBlur` 转成 `GlassPane`（按完整物理矩形，不夹到画面内；`lensed` = 完全在画面内；`edge_band = EDGE × scale`；`body = frame.glass_body(id)`），其余规格原样返回）
- Modify: `fenestra-shell/src/headless.rs`（`render_plan_raster`；新增 `#[doc(hidden)] pub fn backdrop_reads(&self) -> u32` 计数）
- Modify: `fenestra-shell/src/window.rs`（`present_raster_frame` 非 wasm 分支；`WindowShell` 新增调用 `RasterRenderer::glass` 的方法，源纹理用光栅化目标纹理；`frame_stats` 把 GPU 玻璃编码计入 `Filters`，只有真的读回时才计 `Readback`）
- Test: `fenestra-shell/tests/raster_headless.rs`（已有的光栅化无头测试文件）

**Interfaces:**
- Consumes: Task 1–5。两条路径的新流程：背景帧 → `glass_panes` → `raster.glass(…)` 得到占位图片和已合成集合 → 其余规格非空时才读回并 `process_specs_in` → 合并到 `injected` → `paint_final_baked_into(…, &injected, &baked)`。
- `Headless` 加 `#[doc(hidden)] pub fn set_gpu_glass(&mut self, on: bool)`（默认开），关掉时走原 CPU 路径，供对比测试用。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn raster_glass_reads_nothing_back() {
    // A frame with one glass pane over content, Headless::with_backend(Raster):
    // render_plan → backdrop_reads() == 0, and the output's pane region close() to set_gpu_glass(false).
}
#[test]
fn a_pane_partly_off_screen_is_not_stretched() {
    // A pane whose top 40% is above the frame, over horizontal stripes: in the GPU output, the stripe
    // rows inside the visible part sit where the CPU-rendered full frame (pane moved fully on screen,
    // same stripes) puts them — i.e. the stripe period inside the pane equals the backdrop's.
}
#[test]
fn overlapping_panes_each_blur_the_glassless_backdrop() {
    // Two overlapping panes: GPU output close() to set_gpu_glass(false) over both panes.
}
#[test]
fn glass_and_an_element_filter_read_back_only_the_filter() {
    // One glass pane and one element_filter(Blur) element: backdrop_reads() == 1; output close() to CPU.
}
#[test]
fn a_frame_without_glass_is_unchanged() {
    // No glass: backdrop_reads() == 0 and the output equals the one before this task (render twice, compare byte-equal with gpu glass on/off).
}
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test -p fenestra-shell --test raster_headless`；Expected: 新测试编译失败或 FAIL（`backdrop_reads` 为 1）。
- [ ] **Step 3: 实现** 两条路径。
- [ ] **Step 4: 跑测试确认通过** — `cargo test -p fenestra-shell && cargo test -p fenestra`；Expected: 全部 PASS。另跑一次光栅化下的快照测试：`FENESTRA_RENDERER=raster cargo test -p fenestra -p fenestra-kit -p fenestra-looks > raster-goldens.log 2>&1`，失败名单和笔记目录 `raster-golden-failures.txt`（之前的 43 个，全部是抗锯齿差异）对比：不得出现新的失败；玻璃相关的若有变化，逐张看对比图并在账本里记录。
- [ ] **Step 5: 提交** — `git commit -m "feat(shell): raster glass composites on the GPU, with no readback"`

### Task 7: 全量检查、推送、一加实测

- [ ] **Step 1: 本地检查**（fenestra 目录）：
  ```bash
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test -p fenestra-core -p fenestra-raster -p fenestra-shell -p fenestra -p fenestra-kit -p fenestra-looks
  RUSTFLAGS=-Dwarnings cargo check -p fenestra-core -p fenestra-kit -p fenestra-shell -p fenestra -p fenestra-charts -p fenestra-markdown -p fenestra-looks -p fenestra-anim --target wasm32-unknown-unknown
  cargo check --workspace --target aarch64-apple-ios
  ```
  Android（备份并恢复 `Cargo.lock`，设置 NDK 的 `CC_/CXX_/AR_aarch64_linux_android`）：`cargo check --workspace --target aarch64-linux-android`，以及带 winit patch 的 game-activity clippy（`--config 'patch.crates-io.winit.git="https://github.com/paulwang667/winit.git"' --config 'patch.crates-io.winit.rev="18d09344b46c772fdd2b8b3ba318ee7ae58a06ea"' -p fenestra-shell -p fenestra-raster --target aarch64-linux-android --no-default-features --features fenestra-shell/android-game-activity -- -D warnings -A clippy::missing_const_for_thread_local`）。Expected: 全部通过。
- [ ] **Step 2: 推送并触发 CI**（快进推送；`gh workflow run CI --repo paulwang667/fenestra --ref main`）。
- [ ] **Step 3: wechat-ui 升级 fenestra**：`native/` 和 `native/android/` 各 `cargo update -p fenestra`，`cargo test` 全过，`ANDROID_SERIAL=d0305cb2 ./android/build.sh` 安装。
- [ ] **Step 4: 一加实测**（`adb shell setprop debug.wechatui.stats 1`）：通话卡片显示时滚动消息流、长按菜单打开时，记录 FRAMESTATS（`two_pass`、`readback` 应不再出现、`filters`、`render`、`total`、`interval`）；截图对比玻璃外观和之前一致。Expected: 通话卡片显示时滚动的 `interval` p50 ≈ 16.7ms。
- [ ] **Step 5: 提交** wechat-ui 的锁文件升级 — `git commit -m "build: bump fenestra to <sha> (GPU glass)"`

### Task 8: wechat-ui 大面积玻璃试验

**Files:**
- Modify: `wechat-ui/native/src/ui.rs`、`native/src/mobile.rs`、`native/src/fx.rs`（"玻璃主题"）
- Modify: `wechat-ui/native/src/main.rs`、`native/android/src/lib.rs`（开关）
- Test: `wechat-ui/native/tests/mobile.rs`

**Interfaces:**
- `Model` 新字段 `pub glass_theme: bool`（默认 `false`）。开关：桌面读环境变量 `WECHATUI_GLASS=1`；Android 读系统属性 `debug.wechatui.glass=1`。
- 玻璃主题下：根背景铺满 aurora 渐变（`fx::aurora_background`，按窗口尺寸），桌面的侧栏、会话列表、聊天头部、输入栏，以及手机的顶栏、会话列表、tab 栏、聊天顶栏、输入栏，改用 `fx::frosted(…, Glass::…)` 的玻璃样式（复用现有预设）。默认关闭时外观完全不变。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn the_glass_theme_keeps_the_phone_working() {
    // Model with glass_theme = true at 411×800: the phone tests' basics hold (tab bar present,
    // tapping a conversation opens the chat, back returns) — reuse the helpers in tests/mobile.rs.
}
#[test]
fn the_default_theme_is_unchanged() {
    // glass_theme false: render_app at 1280×800 byte-equal to the render before this task (save a reference in the test run, or compare against render with the field untouched).
}
```

- [ ] **Step 2: 跑测试确认失败** — `cargo test --test mobile glass_theme`（在 `native/`）；Expected: 编译失败（`glass_theme` 不存在）。
- [ ] **Step 3: 实现** 玻璃主题和开关。
- [ ] **Step 4: 跑测试确认通过** — `cargo test`；Expected: 全部 PASS。
- [ ] **Step 5: 实测**：
  - Mac：`WECHATUI_GLASS=1 FENESTRA_RENDERER=raster FENESTRA_FRAME_STATS=1 ./target/release/wechat-fui-native`，截图并记录帧时间；
  - 一加 6：`adb shell setprop debug.wechatui.glass 1` 和 `debug.wechatui.stats sync`，首页和聊天页滚动时记录 FRAMESTATS（含 `gpu`），截图。
  - 写进笔记目录 `FRAME-TIME-31MS.md` 新的一节（效果截图路径、帧时间、整屏玻璃的 GPU 开销是否在约 4ms 内）。
- [ ] **Step 6: 提交** — `git commit -m "feat(native): a glass theme (WECHATUI_GLASS / debug.wechatui.glass) to try large glass"`
