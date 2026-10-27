# vello 后端的 GPU 毛玻璃：设计

日期：2026-10-27
仓库：fenestra（`/Users/xxxx/projects/fenestra`，推送到 fork `paulwang667/fenestra` 的 `main`）。试验应用：wechat-ui（`/Users/xxxx/projects/wechat-ui/native`）。
前置工作：光栅化后端的 GPU 玻璃（设计 `2026-10-27-gpu-glass-blur-design.md`，实现到 `09075c5`）。

## 目的
桌面默认用 vello 后端渲染。现在 vello 后端的毛玻璃（`PassKind::BackdropBlur`）每帧要做这几步：
1. 把玻璃要读的区域单独渲染出来（`multi_pass::region_scene`）；
2. **同步读回 CPU**（`device.poll` 等 GPU 画完）；
3. 在 CPU 上模糊、折射；
4. 把结果当普通图片交给最终场景，vello 再用 `write_texture` 把这些像素整块上传回 GPU。

Mac 上 wechat-ui 的玻璃主题有 5 块玻璃，窗口约 2700×2000 物理像素。每帧 9.4ms，其中读回 3～4ms、CPU 滤镜约 2.5ms；同一画面用光栅化后端的 GPU 玻璃只要 4.1ms。

这次让 vello 后端复用光栅化后端已经做好的 GPU 玻璃：在 GPU 上完成模糊、饱和度、折射、本体合成和自适应底色，结果通过 vello 0.9 的 `Renderer::override_image` 直接交给最终场景，**全程不读回、不经 CPU 上传**。

## 目标
- vello 后端的**窗口路径**（主窗口 `present_frame` 和副窗口）里，所有 `BackdropBlur` 都在 GPU 上完成。一帧里只有玻璃时，不发生任何读回。
- 外观与 CPU 版本在容差内一致（沿用光栅化后端 GPU 玻璃的测试容差：超过 8/255 的像素不超过 1%，平均 |Δ| 不超过 1.5/255）。部分在画面外的玻璃按 GPU 路径的方式画：不拉伸，画面外的部分用边缘像素延伸，不做折射。
- 自适应底色在同一帧生效，没有一帧延迟。
- 性能（Mac，wechat-ui 玻璃主题，`FENESTRA_FRAME_STATS=1`）：整帧从约 9.4ms 降到 **≤ 约 5ms**，统计里不再出现 `readback`。没有玻璃的帧不能变慢。
- 光栅化后端和 vello 后端用同一套 GPU 玻璃实现（同样的着色器、图集和 pass），外观一致。

## 非目标
- 网页端（wasm32）不在这次范围内。网页上的 vello 路径继续画没有模糊的玻璃（现有的 `#[cfg(target_arch = "wasm32")]` 分支），只要求能编译。网页端以后单独立项。
- 不改元素自身的滤镜（`ElementFilter`）和自定义 GPU 渲染（`Custom`）：它们仍然读回区域、在 CPU 上处理。一帧里同时有玻璃和这两种时，只为它们读回。
- 不改 vello 的截图基准：无头 vello 默认仍走 CPU 玻璃（见"已经决定的做法"）。
- 不改光栅化后端的行为和外观（只做为了共用而需要的重构）。
- 不做新的玻璃视觉效果。

## 反目标（技术上能跑，但算失败的情况）
- 换成 GPU 以后，vello 窗口里的玻璃看起来明显不同：更糊或更清楚、颜色偏了、圆角处没有折射、边缘有接缝或色带。
- 仍然有同步读回，只是挪了位置。
- 玻璃内容比背景晚一帧或错位。
- 没有玻璃的帧变慢；vello 的截图基准变了；光栅化后端的行为变了。
- 每帧重新分配纹理、重新登记图片，导致内存抖动；或者已经消失的玻璃一直占着 vello 的图片图集。

## 约束
- vello 0.9（`Cargo.toml` 里是 `vello = "0.9.0"`），关键接口：
  - `Renderer::override_image(&ImageData, Option<wgpu::TexelCopyTextureInfoBase<wgpu::Texture>>)`：渲染时，凡是画到这张图片（按 blob id 匹配）的地方，都从给定纹理的 `origin` 处复制一块和图片同样大小的区域进 vello 的图片图集。纹理必须是 `Rgba8Unorm` 并带 `COPY_SRC`。传 `None` 表示注销。
  - 只有被标脏的图片才会重新复制（`override_image` 本身会标脏，另有 `mark_override_image_dirty`），所以每帧内容变了都要重新标脏。
  - fine 着色器支持 `ImageAlphaType::AlphaPremultiplied`（`maybe_premul_alpha`）。
  - vello 的图片图集最大 8192²。这个限制现在的 CPU 路径同样存在，不是新问题。
- 光栅化后端的 GPU 玻璃在 `fenestra-raster/src/glass.rs` 和 `glass.wgsl`：
  - 所有玻璃打包进分页的图集，每帧最多 5 个 render pass；
  - 结果是一张 `Rgba8Unorm` 纹理里的若干子区域；
  - 背景源必须是 `Rgba8Unorm`、带 `TEXTURE_BINDING`，并允许 `Rgba8UnormSrgb` 视图（`view_formats`）。
  
  它现在是 `pub(crate)`，挂在 `RasterRenderer` 内部，通过 `ImageCache::register` 的 uv 子区域交给光栅化后端的最终绘制。
- 光栅化后端已有的"按块的上限"仍然适用：比图集还宽、或超过设备最大纹理尺寸的玻璃，GPU 路径会跳过，只画底色。
- vello 背景区域的渲染目标现在是 `STORAGE_BINDING | COPY_SRC` 的 `Rgba8Unorm` 纹理（窗口路径在 `render_to_image` 里，无头路径在 `Headless::render`/`target` 里）。
- fenestra-core 只能增加公开接口；现有的 `Frame::paint_final_baked_into(target, …, injected, baked)` 已经能用 `SceneTarget` 画出 vello 场景。
- 整个工作区禁止 `unsafe`。必须能在 wasm32、Android、iOS 上编译（CI 的 `check-targets`）。测试按包运行，不跑 `cargo test --workspace`。
- 现有测试和 vello 截图基准全部保持通过。

## 已经决定的做法
- **抽出共用的 `GpuGlass`**：把 `fenestra-raster` 里的 GPU 玻璃变成公开、可以独立使用的类型。它自己持有管线、图集页和占位图片。
  - 输入：设备、队列、背景源纹理、玻璃列表（`GlassPane`）。
  - 输出：每块玻璃的占位 `ImageData`、结果所在的纹理和子区域，以及统计。
  - `RasterRenderer` 在外面包一层，继续把结果登记进自己的图片缓存（行为不变）。vello 后端用同一个类型，把结果交给 `override_image`。
- **背景源就用 vello 已经渲染好的区域纹理**：给它加上 `TEXTURE_BINDING` 和 `Rgba8UnormSrgb` 视图格式，玻璃坐标减去区域原点再交给 `GpuGlass`。
  - 如果某个平台不允许"可作存储绑定的纹理同时声明 sRGB 视图格式"，就退回到先拷一份到独立纹理再读。实现时要在 Mac（Metal）、Linux（Vulkan / lavapipe）和 Windows（DX12，通过 CI）上验证。
- **结果交给 vello**：每帧对每块玻璃调用 `override_image(占位图片, Some(结果纹理 + 子区域原点))`，然后用 `paint_final_baked_into` 生成最终场景：这些玻璃跳过 CPU 合成，直接画占位图片。
  - 上一帧有、这一帧没有的占位图片，要用 `override_image(…, None)` 注销。
  - 结果纹理随图集一起复用，尺寸不变就不重建；纹理换了就重新登记。
- **元素滤镜和自定义渲染照旧**：只有这两类存在时才读回，而且只为它们读回。读回的区域纹理和玻璃用的是同一张，不重复渲染。
- **无头 vello 默认仍走 CPU 玻璃**，保证 vello 截图基准跨平台逐位确定（当初保留 CPU 路径就是为了这个）。现有的 `Headless::set_gpu_glass(bool)` 对 vello 后端也生效，默认值按后端区分：光栅化默认开，vello 默认关。打开以后，无头 vello 走和窗口一样的 GPU 路径，供测试使用。
- **wechat-ui 不需要改代码**。玻璃主题（`WECHATUI_GLASS=1`）用来在 Mac 上实测。

以下几条是设计审查后由我（Claude）决定的，审读时请留意：
- **只在光栅化后端能用的适配器上启用**（Vulkan、Metal、DX12，即 `backend::raster_supported`）。GL/GLES 适配器上的 vello（例如 Android 退回 vello 的情况）继续走 CPU 玻璃，因为 GPU 玻璃用的同类渲染管线在 Adreno 的 GL 驱动上画成过黑色，而且要渲染到 `Rgba16Float`/`Rgba32Float`，GLES 上依赖扩展。所以 vello 窗口路径保留 CPU 玻璃作为第二条路径，按适配器选择。
- **窗口有一个运行时开关强制走 CPU 玻璃**：环境变量 `FENESTRA_GPU_GLASS=0`，对 vello 和光栅化后端都生效，默认是开。它既用于 Mac 上和 CPU 版本对比截图，也是某个平台出问题时的退路。
- **容差怎么比**：GPU 结果图和 CPU 合成的玻璃图（`blur_linear_crop` + `refract_edges` + `bake_glass_body` + `adapted_glass_tint`）比，沿用现有的容差（超过 8/255 的像素 ≤ 1%，平均 |Δ| ≤ 1.5/255）。两条 vello 路径里，玻璃本体都是 fenestra-core 合成好再交给 vello 当图片画的，vello 只负责按圆角画图，所以这一层的比较和光栅化后端相同。整帧对比沿用光栅化无头测试的帧级预算（超过 8/255 的像素 ≤ 2%）。如果 vello 的整帧对比超出预算：先查 vello 画预乘图片的方式（`alpha_type`）；需要改共用着色器时，光栅化后端原有的 CPU 对比测试必须仍然通过；不为 vello 另写一套着色器，也不放宽容差。
- **"消失的玻璃不再占用图集"怎么验收**：检查 fenestra 自己登记过的占位图片集合，以及对它们调用了 `override_image(…, None)`。vello 内部的图集是私有的，不去测它是否真的释放了内存。

## 留给实现者决定
- `GpuGlass` 的具体接口和放在哪里（继续放在 `fenestra-raster`，或者独立成一个小模块），只要光栅化后端的行为不变。
- 占位图片和 `override_image` 登记的生命周期管理（例如按 `WidgetId` 记住上一帧登记过的占位图片）。
- 帧统计里玻璃编码计入哪个阶段（建议沿用光栅化后端的做法，计入 `Filters`）。
- 如果背景区域纹理不能直接作为输入，用什么方式拷贝。
- 无头 vello 打开 GPU 玻璃时，最终帧怎样避免多余的读回（无头 `render` 现在渲染后就读回整张图，作为输出这是必须的；需要避免的是背景区域那一次读回）。

## 验收
- **新测试**（无头 vello 打开 GPU 玻璃，和同一帧的 CPU 版本比较）：
  - 一块玻璃时 `backdrop_reads() == 0`，输出在容差内一致；
  - 多块重叠的玻璃在容差内一致；
  - 玻璃加一个元素滤镜时 `backdrop_reads() == 1`，输出在容差内一致；
  - 部分在画面外的玻璃不被拉伸（复用光栅化后端那个测试的检查方式）；
  - 没有玻璃的帧，打开或关闭 GPU 玻璃时输出逐字节相同；
  - 已经消失的玻璃，它的占位图片会被注销（fenestra 登记的集合里没有它，并且对它调用过 `override_image(…, None)`）；
  - 在 GL 适配器上，或设置了 `FENESTRA_GPU_GLASS=0` 时，走 CPU 玻璃。
- **原有测试**：fenestra 全部原有测试、vello 截图基准（默认的 CPU 路径）、光栅化后端的截图对比都不变；clippy（桌面和 Android）、wasm 和 iOS 编译检查全部通过；推送后 CI 全绿。
- **Mac 实测**：wechat-ui 玻璃主题在 vello 后端下，整帧 ≤ 约 5ms，没有 `readback`。截图和 CPU 版本对比看不出差别。数据记进笔记目录的 `FRAME-TIME-31MS.md`。
