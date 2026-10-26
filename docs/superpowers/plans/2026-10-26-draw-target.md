# DrawTarget：把 fenestra 的绘制从 vello::Scene 解耦 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** fenestra-core 的所有绘制代码改为面向一个后端无关的 `DrawTarget` trait，vello 只是它的一个实现；对现有渲染结果零改变。

**Architecture:** 新增 `fenestra-core/src/draw.rs`，定义 `DrawTarget`（fenestra 实际用到的 8 种绘制操作，再加一个变换栈）和 vello 实现 `SceneTarget`。`painter.rs`、`frame.rs`、`text.rs`、`input.rs` 中所有接收 `&mut Scene` 的函数改为泛型 `T: DrawTarget`。原来用"画进子场景再 `append` 套变换"的地方（节点变换、退场动画的 ghost）改成 `push_transform` / `pop_transform`。对外接口 `Frame::paint() -> Scene`、`paint_backdrop`、`paint_final`、`build_scene` 以及 shell 全部保持不变，另外新增 `Frame::paint_into`。这是两份计划中的第一份；第二份（wgpu 光栅化后端实现 `DrawTarget`）等本计划合入、接口定型后再写。

**Tech Stack:** Rust 2024，vello 0.9.0，peniko 0.6.1，kurbo 0.13.1，parley 0.10.0。

**Spec:** 笔记目录里的 `android-ime-spike/BACKEND-REPORT.md`，"结论"一节和"光栅化后端接入 fenestra 的工作量"表格的第一行（抽 Canvas 接口，4–6 天）。

## Global Constraints

- 只在临时分支 `spike/android-game-activity-ime` 上做（worktree：`scratchpad/fen-wt`），不动 `main`，不推送。
- `unsafe_code = "forbid"`：整个工作区不能用 unsafe。
- 公开 API 里不能有过程宏；所有新增的 `pub` 项都要写 doc 注释（风格与周围代码一致）。
- 渲染结果不能变：**禁止**用 `FENESTRA_UPDATE_SNAPSHOTS=1` 更新快照，所有现有快照必须原样通过。
- 门禁：`cargo fmt --all --check`；`cargo clippy -p fenestra-core -p fenestra-shell -p fenestra --all-targets -- -D warnings`。
- 磁盘：只用一个共享的 `CARGO_TARGET_DIR`。跑测试时设置 `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0`，同一个 `cargo test` 只跑一次，跑完 `rm -rf $CARGO_TARGET_DIR/debug`。禁止 `cargo test --workspace`。
- 现有 `pub mod canvas`（平移缩放画布的几何工具）不改，新接口不能叫 `Canvas`。
- `$SCRATCH` 指会话的 scratchpad 目录（日志、临时文件都放这里，不用 `/tmp`）。

## Review Focus

1. **变换和图层交错**：退场动画的 ghost 同时有缩放（`push_transform`）和透明度（`push_layer`）。图层的裁剪形状必须用 push 时的当前变换，弹出顺序必须和压入顺序一致。期望：画面与改造前一样。（Task 3 测试 `exit_ghost_scales_inside_its_fade_layer`）
2. **变换栈失衡**：`paint_node` 中途提前 `return`（`display: none`、剔除）时，如果已经 `push_transform` 却没 `pop`，后面所有节点都会错位。期望：一帧画完后变换栈深度为 0。（Task 2 的 `SceneTarget::finish` 里 `debug_assert`；Task 3 测试 `transform_stack_is_balanced_after_a_frame`）
3. **伪斜体和可变字体**：`glyph_transform`（合成斜体的 skew）和 `normalized_coords` 必须原样传给后端。期望：斜体和可变字重的文字显示不变。（Task 3 测试 `glyph_runs_carry_skew_and_coords`）
4. **变换节点里的图片和阴影**：`draw_image` 和 `draw_blurred_rounded_rect` 各自带一个变换参数，必须和当前变换组合。期望：旋转或缩放的卡片，阴影和图片跟着转。（Task 3 测试 `transform_wraps_every_op_in_a_transformed_subtree`，场景里包含图片和阴影）
5. **变换节点里的输入框**：`input.rs` 的 `paint` 先压裁剪图层，再画光标、选区和文字，这些都要跟着节点变换。期望：旋转容器里的输入框，裁剪区域和文字对齐。（Task 3 测试 `transform_wraps_every_op_in_a_transformed_subtree`，场景里包含 `raw_input`）

---

### Task 1: `DrawTarget` trait、`SceneTarget` 和测试用的 `Recorder`

**Files:**
- Create: `fenestra-core/src/draw.rs`
- Modify: `fenestra-core/src/lib.rs`（加 `mod draw;`，并 `pub use draw::{DrawTarget, Glyph, GlyphRun, SceneTarget};`）

**Interfaces:**
- Produces（`fenestra_core` 公开）：
  ```rust
  pub struct Glyph { pub id: u32, pub x: f32, pub y: f32 }
  pub struct GlyphRun<'a> {
      pub font: &'a peniko::FontData,
      pub font_size: f32,
      pub hint: bool,
      pub normalized_coords: &'a [i16],
      pub transform: kurbo::Affine,
      pub glyph_transform: Option<kurbo::Affine>,
      pub color: peniko::Color,
  }
  pub trait DrawTarget {
      fn fill<'b>(&mut self, style: peniko::Fill, transform: Affine, brush: impl Into<peniko::BrushRef<'b>>, brush_transform: Option<Affine>, shape: &impl kurbo::Shape);
      fn stroke<'b>(&mut self, style: &kurbo::Stroke, transform: Affine, brush: impl Into<peniko::BrushRef<'b>>, brush_transform: Option<Affine>, shape: &impl kurbo::Shape);
      fn push_layer(&mut self, clip_style: peniko::Fill, alpha: f32, transform: Affine, clip: &impl kurbo::Shape);
      fn push_clip_layer(&mut self, clip_style: peniko::Fill, transform: Affine, clip: &impl kurbo::Shape);
      fn pop_layer(&mut self);
      fn draw_image(&mut self, image: &peniko::ImageData, transform: Affine);
      fn draw_blurred_rounded_rect(&mut self, transform: Affine, rect: kurbo::Rect, color: peniko::Color, radius: f64, std_dev: f64);
      fn draw_glyphs(&mut self, run: &GlyphRun<'_>, glyphs: impl Iterator<Item = Glyph>);
      /// Everything drawn until the matching `pop_transform` is mapped through `transform` (applied after each op's own transform).
      fn push_transform(&mut self, transform: Affine);
      fn pop_transform(&mut self);
  }
  pub struct SceneTarget<'s> { /* scene: &'s mut vello::Scene, stack: Vec<Affine> */ }
  impl<'s> SceneTarget<'s> {
      pub fn new(scene: &'s mut vello::Scene) -> Self;
      /// The composed transform currently applied to every op.
      pub fn current_transform(&self) -> Affine;
      /// Ends painting; debug-asserts every push_transform was popped.
      pub fn finish(self);
  }
  impl DrawTarget for SceneTarget<'_> { … }
  ```
  `push_layer` 固定用 `peniko::Mix::Normal`（fenestra 只用这一种）。
- Produces（仅测试，`#[cfg(test)] pub(crate) mod recorder` 在 `draw.rs` 里）：`Recorder`，实现 `DrawTarget`，记录 `Vec<Op>`。`enum Op { Fill { transform: Affine, bbox: Rect }, Stroke { transform: Affine, bbox: Rect }, PushLayer { alpha: f32, transform: Affine }, PushClip { transform: Affine }, PopLayer, Image { transform: Affine }, Shadow { transform: Affine }, Glyphs { transform: Affine, glyph_transform: Option<Affine>, coords: Vec<i16>, count: usize } }`，其中每个 transform 都已组合上当前变换栈（与 `SceneTarget` 相同的语义）；另有 `fn depth(&self) -> usize` 返回变换栈深度。

- [ ] **Step 1: 写失败的测试**（`draw.rs` 的 `#[cfg(test)] mod tests`）
  ```rust
  #[test]
  fn transform_stack_composes_outer_first() {
      let mut scene = vello::Scene::new();
      let mut t = SceneTarget::new(&mut scene);
      let a = Affine::translate((10.0, 0.0));
      let b = Affine::scale(2.0);
      t.push_transform(a);
      t.push_transform(b);
      assert_eq!(t.current_transform(), a * b);
      t.pop_transform();
      assert_eq!(t.current_transform(), a);
      t.pop_transform();
      assert_eq!(t.current_transform(), Affine::IDENTITY);
      t.finish();
  }

  #[test]
  #[should_panic]
  #[cfg(debug_assertions)]
  fn finish_panics_on_an_unpopped_transform() {
      let mut scene = vello::Scene::new();
      let mut t = SceneTarget::new(&mut scene);
      t.push_transform(Affine::scale(2.0));
      t.finish();
  }

  #[test]
  fn recorder_composes_like_scene_target() {
      let mut r = recorder::Recorder::default();
      r.push_transform(Affine::translate((5.0, 5.0)));
      r.fill(peniko::Fill::NonZero, Affine::scale(2.0), peniko::Color::BLACK, None, &kurbo::Rect::new(0.0, 0.0, 1.0, 1.0));
      r.pop_transform();
      assert_eq!(r.depth(), 0);
      assert!(matches!(r.ops[0], recorder::Op::Fill { transform, .. } if transform == Affine::translate((5.0, 5.0)) * Affine::scale(2.0)));
  }
  ```
- [ ] **Step 2: 运行确认失败**
  Run: `cargo test -p fenestra-core --lib draw::` → Expected: 编译失败（`SceneTarget` 未定义）。
- [ ] **Step 3: 实现 `draw.rs`**。`SceneTarget` 每个操作都把参数里的 `transform` 换成 `self.current_transform() * transform` 再调用 vello 对应的方法；`draw_glyphs` 把 `run.transform` 同样组合后，依次调用 `scene.draw_glyphs(run.font).brush(run.color).hint(run.hint).transform(..).glyph_transform(run.glyph_transform).font_size(run.font_size).normalized_coords(run.normalized_coords).draw(Fill::NonZero, glyphs.map(|g| vello::Glyph { id: g.id, x: g.x, y: g.y }))`。`push_transform` 压入 `current * transform`。
- [ ] **Step 4: 运行确认通过**
  Run: `cargo test -p fenestra-core --lib draw::` → Expected: 3 passed（release 构建下 `finish_panics…` 不编译）。
- [ ] **Step 5: 门禁并提交**
  Run: `cargo fmt --all --check && cargo clippy -p fenestra-core --all-targets -- -D warnings`
  ```bash
  git add fenestra-core/src/draw.rs fenestra-core/src/lib.rs
  git commit -m "feat(core): DrawTarget, a backend-neutral drawing interface"
  ```

### Task 2: 绘制代码改为面向 `DrawTarget`（行为不变）

**Files:**
- Modify: `fenestra-core/src/painter.rs`（第 351、457、491、567、585、762、810、838、871 行的 9 个函数）
- Modify: `fenestra-core/src/text.rs`（`Fonts::paint_rich` 第 788 行、`Fonts::paint` 第 963 行）
- Modify: `fenestra-core/src/input.rs`（`paint` 第 928 行、`paint_masked` 第 1070 行、`draw_text` 第 1190 行）
- Modify: `fenestra-core/src/frame.rs`（`paint_with` 2265、`paint_exits` 2311、`paint_ghost_node` 2366、`paint_ghost_node_unscaled` 2383、`paint_node` 2433、`paint_node_unscaled` 2467）

**Interfaces:**
- Consumes: Task 1 的 `DrawTarget`、`GlyphRun`、`Glyph`、`SceneTarget`。
- Produces: 上述每个函数的 `scene: &mut Scene` 参数改为 `target: &mut T`，加泛型 `<T: DrawTarget>`（其余参数和返回值不变）。`Frame::paint_with` 的返回值仍是 `Scene`，内部做法：`let mut scene = Scene::new(); let mut t = SceneTarget::new(&mut scene); /* 原有绘制，传 &mut t */ t.finish(); scene`。

- [ ] **Step 1: 记录基线**：在改动前跑一遍，确认现有测试全绿（这是本任务的"失败测试"：改动过程中任何快照失败都说明行为变了）。
  Run: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 cargo test -p fenestra-core -p fenestra-shell -p fenestra -p fenestra-kit -p fenestra-looks > $SCRATCH/base.log 2>&1; grep -c "^test result: ok" $SCRATCH/base.log` → Expected: 全部 ok，0 failed。
- [ ] **Step 2: painter.rs**：9 个函数改成泛型；函数体里 `scene.fill/stroke/push_layer/push_clip_layer/pop_layer/draw_image/draw_blurred_rounded_rect` 改调 `target` 上的同名方法（`push_layer` 去掉 `peniko::Mix::Normal` 实参）。删掉 `use vello::Scene;`。
- [ ] **Step 3: text.rs 和 input.rs**：`draw_glyphs(...)` 链式调用改为构造 `GlyphRun { font: run.font(), font_size: run.font_size(), hint: true, normalized_coords: run.normalized_coords(), transform, glyph_transform: glyph_xform, color }`，再调 `target.draw_glyphs(&run, …map(|g| Glyph { id, x, y }))`。删掉 `use vello::Scene;`。
- [ ] **Step 4: frame.rs**：6 个函数改成泛型。三处 `let mut sub = Scene::new(); … scene.append(&sub, Some(a));`（第 2344–2352、2371–2373、2460–2462 行）改为 `target.push_transform(a); …; target.pop_transform();`，`push`/`pop` 之间不允许提前 `return`。`paint_with` 按 Interfaces 里的写法包一层 `SceneTarget`。`frame.rs` 里除 `paint_with` / `build_scene` / 公开方法签名以外，不再出现 `Scene` 类型。
- [ ] **Step 5: 运行全部测试，与基线对比**
  Run: Step 1 的同一条命令（输出到 `$SCRATCH/after.log`），然后 `grep -cE "FAILED|panicked" $SCRATCH/after.log` → Expected: `0`，`ok` 的行数和基线相同。
- [ ] **Step 6: 门禁、清理、提交**
  Run: `cargo fmt --all --check && cargo clippy -p fenestra-core -p fenestra-shell -p fenestra --all-targets -- -D warnings && rm -rf $CARGO_TARGET_DIR/debug`
  ```bash
  git add fenestra-core/src/painter.rs fenestra-core/src/text.rs fenestra-core/src/input.rs fenestra-core/src/frame.rs
  git commit -m "refactor(core): paint through DrawTarget; transforms via push_transform"
  ```

### Task 3: `Frame::paint_into` 和非 vello 后端的回归测试

**Files:**
- Modify: `fenestra-core/src/frame.rs`（新增公开方法；`paint` 改为调用它）
- Test: `fenestra-core/src/draw.rs` 的 `tests` 模块（用 `recorder::Recorder`）

**Interfaces:**
- Consumes: Task 1 的 `Recorder` / `Op`，Task 2 的泛型绘制函数。
- Produces:
  ```rust
  impl Frame {
      /// Paints the frame (single pass, no filtered regions) into any DrawTarget.
      pub fn paint_into<T: DrawTarget>(&self, target: &mut T, fonts: &mut Fonts, state: &mut FrameState);
  }
  ```
  `Frame::paint` 的实现改为 `Scene::new()` + `SceneTarget` + `paint_into` + `finish`。第二份计划里的光栅化后端就是通过它接入的。

- [ ] **Step 1: 写失败的测试**（`draw.rs` 的 tests；`Recorder` 的 `Op` 增加 `fn transform(&self) -> Option<Affine>`，`PopLayer` 返回 `None`）
  ```rust
  /// Rotation of an affine's linear part, in degrees (unaffected by a positive scale).
  fn angle_deg(a: Affine) -> f64 {
      let c = a.as_coeffs();
      c[1].atan2(c[0]).to_degrees()
  }

  /// A container rotated 30° holding a shadowed half-opacity card, an image,
  /// synthesized italic text, and an input field.
  fn rotated_fixture() -> (Frame, Fonts, FrameState) {
      let root = col::<()>().w(300.0).h(400.0).rotate(30.0).children([
          col::<()>().w(200.0).h(80.0).bg(Color::WHITE).rounded(12.0).shadow(ShadowToken::Md).opacity(0.5),
          image_rgba8(4, 4, vec![255; 64]).w(40.0).h(40.0),
          text("italic").italic(),
          raw_input("hello", "").id("field").w(200.0).h(32.0),
      ]);
      let mut fonts = Fonts::embedded();
      let mut state = FrameState::new();
      let frame = build_frame(&root, &Theme::light(), &mut fonts, &mut state, (400.0, 500.0), 1.0);
      (frame, fonts, state)
  }

  #[test]
  fn transform_wraps_every_op_in_a_transformed_subtree() {
      let (frame, mut fonts, mut state) = rotated_fixture();
      let mut r = Recorder::default();
      frame.paint_into(&mut r, &mut fonts, &mut state);
      for op in &r.ops {
          if let Some(t) = op.transform() {
              assert!((angle_deg(t) - 30.0).abs() < 1e-3, "{op:?} not rotated with its node");
          }
      }
      assert!(r.ops.iter().any(|o| matches!(o, Op::Image { .. })));
      assert!(r.ops.iter().any(|o| matches!(o, Op::Shadow { .. })));
      assert!(r.ops.iter().any(|o| matches!(o, Op::PushLayer { alpha, .. } if (*alpha - 0.5).abs() < 1e-6)));
      assert!(r.ops.iter().any(|o| matches!(o, Op::PushClip { .. })));
  }

  #[test]
  fn transform_stack_is_balanced_after_a_frame() {
      let (frame, mut fonts, mut state) = rotated_fixture();
      let mut r = Recorder::default();
      frame.paint_into(&mut r, &mut fonts, &mut state);
      assert_eq!(r.depth(), 0);
      let pushes = r.ops.iter().filter(|o| matches!(o, Op::PushLayer { .. } | Op::PushClip { .. })).count();
      let pops = r.ops.iter().filter(|o| matches!(o, Op::PopLayer)).count();
      assert_eq!(pushes, pops);
  }

  #[test]
  fn glyph_runs_carry_skew_and_coords() {
      let (frame, mut fonts, mut state) = rotated_fixture();
      let mut r = Recorder::default();
      frame.paint_into(&mut r, &mut fonts, &mut state);
      // Fonts::embedded() has no italic face, so .italic() is synthesized as a skew.
      assert!(r.ops.iter().any(|o| matches!(o, Op::Glyphs { glyph_transform: Some(_), .. })));
      assert!(r.ops.iter().all(|o| !matches!(o, Op::Glyphs { count: 0, .. })));
  }

  #[test]
  fn exit_ghost_scales_inside_its_fade_layer() {
      // tests/motion.rs `exit_view`, with the ghost also scaling to 0.5.
      let view = |present: bool| {
          let mut kids = vec![div::<()>().id("keep").w(100.0).h(20.0).shrink0()];
          if present {
              kids.push(div::<()>().id("gone").w(100.0).h(20.0).shrink0().bg(Color::BLACK).exit_to(0.0, 0.5, 0.0, 0.0));
          }
          col::<()>().w(200.0).h(400.0).children(kids)
      };
      let theme = Theme::light();
      let mut fonts = Fonts::embedded();
      let mut state = FrameState::new();
      let _ = build_frame(&view(true), &theme, &mut fonts, &mut state, (400.0, 500.0), 1.0);
      let f2 = build_frame(&view(false), &theme, &mut fonts, &mut state, (400.0, 500.0), 1.0);
      state.tick(0.05); // mid-exit
      let mut r = Recorder::default();
      f2.paint_into(&mut r, &mut fonts, &mut state);
      let layer = r.ops.iter().position(|o| matches!(o, Op::PushLayer { alpha, .. } if *alpha < 1.0)).expect("the ghost fades in a layer");
      let scaled = r.ops[layer + 1..].iter().find_map(|o| o.transform()).expect("the ghost paints inside it");
      assert!(scaled.as_coeffs()[0] < 1.0 && scaled.as_coeffs()[0] > 0.5, "mid-exit scale between 1 and 0.5");
      assert_eq!(r.depth(), 0);
  }
  ```
- [ ] **Step 2: 运行确认失败**
  Run: `cargo test -p fenestra-core --lib draw::` → Expected: 编译失败（`paint_into` 未定义）。
- [ ] **Step 3: 实现 `Frame::paint_into`**，`Frame::paint` 改为调用它。
- [ ] **Step 4: 运行确认通过**
  Run: `cargo test -p fenestra-core --lib draw::` → Expected: 7 passed（Task 1 的 3 个加本任务 4 个）。
- [ ] **Step 5: 全量回归、门禁、清理、提交**
  Run: Task 2 Step 1 的同一条测试命令 → Expected: 0 failed；`cargo fmt --all --check`；`cargo clippy -p fenestra-core -p fenestra-shell -p fenestra --all-targets -- -D warnings`；`rm -rf $CARGO_TARGET_DIR/debug`。
  ```bash
  git add fenestra-core/src/frame.rs fenestra-core/src/draw.rs
  git commit -m "feat(core): Frame::paint_into, painting into any DrawTarget"
  ```

### Task 4: 真机冒烟测试

**Files:** 无代码改动（用探针工程 `scratchpad/fenprobe`，它依赖 `fen-wt` 下的 fenestra）。

- [ ] **Step 1**: `CARGO_TARGET_DIR=<共享目录> ./build.sh` 后用 gradle 打包，安装到华为 `TNL0220A13019087`，依次用 `debug.fenprobe.scene` = `static`、`vlist`、`ime` 启动。
- [ ] **Step 2**: 每个场景截图，和改造前的截图比较（`static` 的卡片、阴影、圆角，`vlist` 的头像和文字，`ime` 的中文和输入框光标）。Expected: 肉眼无差别；`FRAMESTATS` 的 paint 耗时与改造前同量级（static 约 1.3ms）。
- [ ] **Step 3**: 把结论写进笔记目录的 `BACKEND-REPORT.md`，新增一节"DrawTarget 已落地"，写清提交号和截图对比结论。
