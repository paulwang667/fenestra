# vello 后端的 GPU 毛玻璃 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**目标：** vello 后端在窗口里用光栅化后端那套 GPU 玻璃完成毛玻璃，结果经 `Renderer::override_image` 交给最终场景，只有玻璃时不再读回。

**架构：** 把 `fenestra-raster` 的 GPU 玻璃抽成公开的 `GpuGlass`（光栅化后端包一层，行为不变）。`fenestra-shell` 新增 `VelloGlass`：在 vello 渲染好的背景区域纹理上运行 `GpuGlass`，并负责 `override_image` 的登记和注销。无头 vello 默认仍走 CPU 玻璃，打开开关后走这条路径，供测试使用；窗口路径（主窗口和副窗口）默认走这条路径。启用条件：适配器是 Vulkan/Metal/DX12，且 `FENESTRA_GPU_GLASS` 不是 `0`。

**技术栈：** Rust，wgpu 29，vello 0.9（`override_image`），fenestra-raster 的 `glass.rs` / `glass.wgsl`。

**设计文档：** `docs/superpowers/specs/2026-10-27-vello-gpu-glass-design.md`

## 全局约束
- vello 0.9：`override_image(&ImageData, Option<wgpu::TexelCopyTextureInfoBase<wgpu::Texture>>)`。纹理必须是 `Rgba8Unorm` 并带 `COPY_SRC`。每帧内容变了都要重新调用（它会把图片标脏）；传 `None` 表示注销。
- 只在 `backend::raster_supported(adapter)` 为真（Vulkan、Metal、DX12）时启用；GL/GLES 上的 vello 继续走 CPU 玻璃。
- 环境变量 `FENESTRA_GPU_GLASS=0` 让窗口强制走 CPU 玻璃，对 vello 和光栅化后端都生效，默认开。
- 无头：`Headless::set_gpu_glass(bool)` 对两个后端都生效。默认值：光栅化为 `true`，vello 为 `false`（保证 vello 截图基准逐位不变）。
- 对比容差：玻璃结果图和 CPU 版本比，超过 8/255 的像素 ≤ 1%，平均 |Δ| ≤ 1.5/255；整帧比，超过 8/255 的像素 ≤ 2%。不为 vello 另写着色器；共用着色器如果要改，光栅化后端原有的 `gpu_glass_tests` 必须全部通过。
- 光栅化后端的行为和外观不变。网页端（wasm32）继续画没有模糊的玻璃，只要求能编译。
- 禁止 `unsafe`；测试按包运行，不跑 `cargo test --workspace`；统一使用一个共享的 `CARGO_TARGET_DIR`。

## 审查重点
1. 一帧里玻璃消失了：上一帧登记过的占位图片要立刻用 `override_image(…, None)` 注销，不能一直占着 vello 的图片图集（Task 2 测试 `a_vanished_pane_is_unregistered_from_vello`）。
2. 背景区域纹理同时用作 vello 的存储目标和 GPU 玻璃的 sRGB 输入：Metal、Vulkan 都要能跑；CI 上的 lavapipe 也要能跑（Task 2 的测试在 CI 的 Linux 和 macOS 作业里运行）。
3. 窗口大小变了、区域原点变了：玻璃坐标要按这一帧的区域原点平移，不能用上一帧的（Task 2 测试 `a_pane_under_a_moved_region_lines_up`）。
4. 同一帧既有玻璃又有元素滤镜：只读回一次，而且读的就是给玻璃用的那张区域纹理，不重复渲染（Task 2 测试 `glass_and_an_element_filter_read_back_only_the_filter`）。
5. `FENESTRA_GPU_GLASS=0` 时，两个后端的窗口都走完整的 CPU 路径（Task 3 的单元测试 `gpu_glass_switch_reads_the_environment`）。

---

### Task 1: 公开的 `GpuGlass`

**Files:**
- Modify: `fenestra-raster/src/glass.rs`（`pub(crate) struct Glass` 改为 `pub struct GpuGlass`，并加 `GlassResult`）
- Modify: `fenestra-raster/src/lib.rs`（`RasterRenderer` 改用 `GpuGlass`；导出 `GpuGlass`、`GlassResult`）
- Test: `fenestra-shell/src/gpu_glass_tests.rs`

**Interfaces:**
- Produces（供 Task 2 使用）：
  - `pub struct GlassResult { pub id: WidgetId, pub placeholder: peniko::ImageData, pub texture: wgpu::Texture, pub region: [u32; 4] /* x, y, w, h */, pub uv: [f32; 4] }`
  - `#[derive(Default)] pub struct GpuGlass`
    - `pub fn run(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Texture, panes: &[GlassPane]) -> (Vec<GlassResult>, GlassStats)`
    - `pub fn len(&self) -> usize`
    - `pub fn is_empty(&self) -> bool`
    - `pub fn result_of(&self, placeholder: &ImageData) -> Option<(&wgpu::Texture, [u32; 4])>`
  - `GlassResult.texture` 是图集结果纹理的克隆（`wgpu::Texture` 可以廉价克隆）。`RasterRenderer::glass` 的签名和行为都不变。

- [ ] **Step 1: 写失败的测试**（加到 `gpu_glass_tests.rs`）

```rust
#[test]
fn gpu_glass_runs_on_its_own() {
    // GpuGlass::default().run(device, queue, &source, &[p]) with p = pane((60, 40, 200, 160), 12.0, 1.5)
    // → one GlassResult for p.id; crate::headless::read_texture(device, queue, &r.texture, (r.region[0], r.region[1], r.region[2], r.region[3]))
    //   assert_close(.., &cpu_blur(&img, &p), "standalone"); and r.placeholder.width/height == (200, 160).
}
```

- [ ] **Step 2: 运行，确认失败** — `cargo test -p fenestra-shell --lib gpu_glass_runs_on_its_own`；预期：编译失败（`GpuGlass` 不存在）。
- [ ] **Step 3: 实现** — 把 `Glass` 改名为公开的 `GpuGlass`，`run` 返回 `Vec<GlassResult>`。`RasterRenderer::glass` 拿 `uv` 和纹理视图去调用 `images.register`，`read_registered` 改用 `result_of`。
- [ ] **Step 4: 运行，确认通过** — `cargo test -p fenestra-shell --lib gpu_glass_tests && cargo test -p fenestra-raster`；预期：全部通过（原有 13 个加上新的 1 个）。
- [ ] **Step 5: 提交** — `git commit -m "refactor(raster): GpuGlass, usable without a RasterRenderer"`

### Task 2: `VelloGlass` 和无头 vello 的 GPU 路径

**Files:**
- Create: `fenestra-shell/src/vello_glass.rs`（在 `lib.rs` 里 `#[cfg(not(target_arch = "wasm32"))] mod vello_glass;`）
- Modify: `fenestra-shell/src/headless.rs`（vello 分支的 `render_plan`、`gpu_glass` 的默认值、`set_gpu_glass`、新的调试接口）
- Modify: `fenestra-shell/src/multi_pass.rs`（`glass_panes` 增加区域原点参数，或新增一个把玻璃平移到区域坐标的辅助函数）
- Test: `fenestra-shell/tests/vello_gpu_glass.rs`（新建；场景照搬 `tests/raster_headless.rs` 里的 `view`/`bands`/`glass_pane`）

**Interfaces:**
- Consumes：Task 1 的 `GpuGlass`、`GlassResult`。
- Produces（供 Task 3 使用）：
  - `pub(crate) struct VelloGlass`（`Default`）
    - `pub(crate) fn composite(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut vello::Renderer, source: &wgpu::Texture, origin: (u32, u32), panes: &[GlassPane]) -> HashMap<WidgetId, ImageData>`
    - `pub(crate) fn registered(&self) -> usize`
  - `composite` 的约定：
    - `panes` 用帧坐标，内部减去 `origin` 再交给 `GpuGlass`；
    - 对每个结果调用 `override_image(&placeholder, Some(TexelCopyTextureInfoBase { texture, mip_level: 0, origin: Origin3d { x, y, z: 0 }, aspect: All }))`；
    - 上一帧登记过、这一帧没有的占位图片，用 `override_image(…, None)` 注销；
    - 返回 id 到占位图片的映射，作为 `paint_final_baked_into` 的 `injected`，并用它的 keys 作为 `baked`。
  - `pub(crate) fn render_target(device: &wgpu::Device, (w, h): (u32, u32)) -> wgpu::Texture`：创建背景区域纹理，`Rgba8Unorm`，用途 `STORAGE_BINDING | TEXTURE_BINDING | COPY_SRC`，`view_formats: [Rgba8UnormSrgb]`。窗口和无头都用它。
  - `Headless`：
    - `gpu_glass` 默认值改为 `backend == Backend::Raster`；
    - `#[doc(hidden)] pub fn vello_glass_registered(&self) -> usize`。
  - 无头 vello 的 GPU 路径只在 `gpu_glass && raster_supported(adapter)` 时启用：
    1. 把 `region_scene` 渲染进 `render_target`（不读回）；
    2. 调用 `VelloGlass::composite`；
    3. 只有存在非玻璃规格时，才对同一张纹理调用 `read_texture`，并让 `backdrop_reads += 1`；
    4. 最终场景用 `SceneTarget::new(&mut scene)` 加 `paint_final_baked_into` 生成，再像现在一样 `render` 输出。

- [ ] **Step 1: 写失败的测试**（新文件 `tests/vello_gpu_glass.rs`。辅助函数 `vello(view, gpu_glass: bool) -> (RgbaImage, u32 reads)` 走 `Headless::with_backend(Backend::Vello)` 加 `set_gpu_glass`）

```rust
#[test] fn vello_headless_keeps_cpu_glass_by_default()      // default Headless (Vello): one glass pane → backdrop_reads() == 1
#[test] fn vello_gpu_glass_reads_nothing_back()             // gpu on: reads == 0; frame close to gpu off (≤ 2% of pixels over 8/255)
#[test] fn overlapping_panes_each_blur_the_glassless_backdrop() // two overlapping panes, gpu on vs off: close; reads == 0
#[test] fn glass_and_an_element_filter_read_back_only_the_filter() // one pane + text with ElementFilter::Blur(2.0): reads == 1; close
#[test] fn a_pane_partly_off_screen_is_not_stretched()      // the raster test's band-correlation check (> 0.8), on vello
#[test] fn a_pane_under_a_moved_region_lines_up()           // render a frame, then the same view with the pane moved 80px right:
                                                            // the second frame close to its own CPU render (stale origin would shift it)
#[test] fn a_frame_without_glass_is_unchanged()             // no glass: byte-equal with gpu on and off; reads == 0
#[test] fn a_vanished_pane_is_unregistered_from_vello()     // same Headless: frame with glass → vello_glass_registered() == 1;
                                                            // then a frame without it → == 0
```

- [ ] **Step 2: 运行，确认失败** — `cargo test -p fenestra-shell --test vello_gpu_glass`；预期：编译失败（`vello_glass_registered` 不存在），或者第一项以外全部失败（读回次数为 1）。
- [ ] **Step 3: 实现** `vello_glass.rs` 和无头 vello 的 GPU 路径（按上面 Interfaces 的约定）。
- [ ] **Step 4: 运行，确认通过** — `cargo test -p fenestra-shell && cargo test -p fenestra`；预期：全部通过，vello 截图基准不变（默认路径没动）。
- [ ] **Step 5: 提交** — `git commit -m "feat(shell): headless vello can composite glass on the GPU"`

### Task 3: 窗口路径（主窗口和副窗口）和 `FENESTRA_GPU_GLASS`

**Files:**
- Modify: `fenestra-shell/src/window.rs`：
  - `render_to_image` 拆成两步：`render_region(scene, w, h, base) -> Option<Result<(), ShellError>>` 只渲染到 `Readback` 纹理（改用 `vello_glass::render_target` 创建），`read_region() -> Option<Result<RgbaImage, ShellError>>` 读回同一张纹理。
  - 主窗口 `present_frame` 和副窗口的两遍渲染分支都接上 `VelloGlass`。
  - `present_raster_frame` 在开关关闭时走全 CPU：所有规格都交给 `process_specs_in`。
- Modify: `fenestra-shell/src/backend.rs`：新增 `pub(crate) fn gpu_glass_enabled() -> bool`。读一次环境变量 `FENESTRA_GPU_GLASS`，`"0"` 表示关，其余值或未设置表示开。
- Test: `fenestra-shell/src/backend.rs` 的测试模块

**Interfaces:**
- Consumes：Task 2 的 `VelloGlass::composite`、`render_target`、`glass_panes`。
- `WindowShell` 新增字段 `#[cfg(not(target_arch = "wasm32"))] vello_glass: crate::vello_glass::VelloGlass`（每个窗口一个；副窗口用各自的 `bundle.shell`）。
- 启用条件：`gpu_glass_enabled() && raster_supported(当前 surface 的适配器后端)`。否则保持现有 CPU 代码路径，不做任何改动。
- 帧统计：GPU 玻璃的编码计入 `Phase::Filters`；只有真的读回时才计 `Phase::Readback`。

- [ ] **Step 1: 写失败的测试**

```rust
#[test]
fn gpu_glass_switch_reads_the_environment() {
    // a pure helper gpu_glass_from(value: Option<&str>) -> bool behind gpu_glass_enabled():
    // None → true, Some("1") → true, Some("") → true, Some("0") → false
}
```

- [ ] **Step 2: 运行，确认失败** — `cargo test -p fenestra-shell --lib gpu_glass_switch`；预期：编译失败。
- [ ] **Step 3: 实现** `gpu_glass_from`、`gpu_glass_enabled`，以及上面列出的窗口接线。
- [ ] **Step 4: 运行，确认通过** — `cargo test -p fenestra-shell && cargo clippy -p fenestra-shell --all-targets -- -D warnings`；预期：全部通过。另外用 `RUSTFLAGS=-Dwarnings cargo check -p fenestra-shell --target wasm32-unknown-unknown` 确认 wasm 能编译。
- [ ] **Step 5: 提交** — `git commit -m "feat(shell): vello windows composite glass on the GPU (FENESTRA_GPU_GLASS=0 opts out)"`

### Task 4: 全量检查、推送、Mac 实测

- [ ] **Step 1: 本地检查**（命令同上一份计划的 Task 7）：
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test -p fenestra-core -p fenestra-raster -p fenestra-shell -p fenestra -p fenestra-kit -p fenestra-looks`
  - 光栅化截图对比：`FENESTRA_RENDERER=raster FENESTRA_SNAPSHOT_BUDGET=0.02 …`，和基线的 33 个失败比较，不能有新增；
  - wasm、iOS、Android 的 check，以及带 winit patch 的 game-activity clippy。
  
  预期：全部通过；vello 截图基准不变。
- [ ] **Step 2: 推送并触发 CI** — 快进推送到 `fork/main`，然后 `gh workflow run CI --repo paulwang667/fenestra --ref main`。一次推送完所有提交，避免 `cancel-in-progress` 取消掉正在跑的 CI。
- [ ] **Step 3: wechat-ui 升级 fenestra 并实测**：
  - `native/` 和 `native/android/` 各执行 `cargo update -p fenestra`，`cargo test` 全部通过；
  - 构建 release，分别运行：
    - `WECHATUI_GLASS=1 FENESTRA_FRAME_STATS=1`（vello 默认）
    - `WECHATUI_GLASS=1 FENESTRA_FRAME_STATS=1 FENESTRA_GPU_GLASS=0`
  - 记录 FRAMESTATS，截图只裁到应用窗口，两张对比外观。
  
  预期：GPU 路径整帧 ≤ 约 5ms，没有 `readback`；两张截图看不出差别。
- [ ] **Step 4: 记录并提交** — 数据写进笔记目录 `FRAME-TIME-31MS.md`；wechat-ui 提交 `build: bump fenestra to <sha> (vello GPU glass)`（只在本地，不推送）。
