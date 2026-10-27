# 具名材质风格 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**目标：** 把实色、液态玻璃、亚克力、Material 色调四种具名风格做成 fenestra 主题的一部分。kit 组件和按 `Surface` 角色标注的表面换风格时自动跟着变；wechat-ui 改用 fenestra 的实现。

**架构：**
- `MaterialStyle` 和 `reduce_transparency` 是 `Theme` 的字段。
- 唯一的解析入口是 `Theme::apply_surface(role, base) -> Style`，`Theme::surface_style` 和 `Element::surface` 都调用它。每种风格的配方集中在 `fenestra-core/src/surface.rs`。
- kit 新增 `app_bar`、`tab_bar`、`scaffold`。
- 因为 `view()` 里没有主题，`scaffold` 的元素树结构固定：栏是绝对定位的层，由 themed 闭包按风格决定内容的内边距加在外层（栏在内容旁边，不覆盖），还是加在滚动内容上（内容从栏下面滚过），以及底栏是否悬浮。这样切换风格时元素身份不变。

**技术栈：** Rust，fenestra-core（`Surface`、`Theme`、`Style`、`Material`），fenestra-kit，fenestra-looks，fenestra-shell（`assert_png_snapshot`）。

**设计文档：** `docs/superpowers/specs/2026-10-27-material-styles-design.md`

## 全局约束
- 默认风格 `Solid` 下，所有已有角色的解析结果和现在完全相同；现有截图基准全部不变。
- 配方的圆角对应已有级别（`R_SM` 6、`R_MD` 10、`R_LG` 14、`R_XL` 20、`R_FULL`），通过 `theme.radius`（`RadiusScale`）解析；不给 `RadiusScale` 加字段。
- `Elevation::Flat` 下，静止的 `Card`、`Raised` 不要阴影（所有风格都是）。
- `reduce_transparency` 打开时，任何角色都没有模糊、填充不透明；`ThemeSpec` 的 `DeriveSpec.contrast == Contrast::High` 推导出的主题自动打开它。
- 活力文字色只在 `LiquidGlass` 的玻璃角色上，并且没有打开 `reduce_transparency` 时才用。
- 只能增加公开接口（`Theme` 加字段要写进 CHANGELOG）；禁止 `unsafe`；能在 wasm32、Android、iOS 上编译；测试按包运行，不跑 `cargo test --workspace`；使用共享的 `CARGO_TARGET_DIR`。
- wechat-ui 只在本地提交，不推送；不要在 wechat-ui 里运行 `cargo fmt`。

## 审查重点
1. 运行时切换风格：`scaffold` 里的输入框焦点和滚动位置都要保留（Task 4 测试 `switching_style_keeps_focus_and_scroll`）。
2. `RadiusScale::sharp()` 主题加非实色风格：配方圆角跟着变小（Task 2 测试 `recipes_follow_the_radius_scale`）。
3. `reduce_transparency` 加 `LiquidGlass`：`label_on` 返回普通文字色（Task 2 测试 `reduced_transparency_drops_vibrant_labels`）。
4. `Elevation::Flat` 加任意风格：`Card` 没有阴影（Task 2 测试 `flat_elevation_holds_in_every_style`）。
5. 浅色模式：活力文字色是黑色系，Mica 底色基于浅色页面（Task 2 测试 `recipes_resolve_in_light_and_dark`）。

---

### Task 1: `MaterialStyle`、主题字段、新角色、唯一的解析入口

**Files:**
- Modify: `fenestra-core/src/surface.rs`、`theme.rs`、`element.rs`（`surface()`）、`lib.rs`（导出）
- Test: `fenestra-core/src/surface.rs` 的测试模块

**Interfaces:**
- Produces（后续任务都会用到）：
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)] pub enum MaterialStyle { #[default] Solid, LiquidGlass, Acrylic, Tonal }`
    - `pub fn bars_overlay_content(self) -> bool`：`LiquidGlass` 或 `Acrylic` 为 true；
    - `pub fn floating_dock(self) -> bool`：只有 `LiquidGlass` 为 true；
    - `pub const ALL: [Self; 4]`。
  - `Theme`：
    - 新字段 `pub material_style: MaterialStyle`、`pub reduce_transparency: bool`（默认 Solid 和 false）；
    - `pub fn with_material_style(self, MaterialStyle) -> Self`；
    - `pub fn with_reduce_transparency(self, bool) -> Self`；
    - `pub fn apply_surface(&self, role: Surface, base: Style) -> Style`：唯一的解析入口；
    - `surface_style(role)` 改成 `self.apply_surface(role, Style::default())`。
  - `Surface` 新变体：`Bar`、`Dock`、`Window`。
  - `ThemeSpec` 新字段：`#[serde(default)] pub material_style: MaterialStyle`，`ThemeSpec::theme()` 会应用它。

- [ ] **Step 1: 写失败的测试**

```rust
#[test] fn solid_resolution_is_unchanged()
// For every pre-existing role and Theme::{light,dark}(), RadiusScale default and sharp, Elevation default and Flat:
// theme.surface_style(role) == legacy(theme, role), where legacy is today's code path
// (role.bundle().apply(theme, Style::default()).rounded(role.radius_px(&theme.radius)) + the Flat shadow rule) kept as a test helper.
#[test] fn solid_bar_dock_and_window()
// Solid Bar and Dock: fill == theme.surface, border 1px border_subtle, no backdrop_blur, no shadow;
// Window: fill == theme.bg, no border, radius 0.
#[test] fn material_style_round_trips_through_the_theme_and_spec()
// Theme::dark().material_style == Solid; .with_material_style(Acrylic).material_style == Acrylic;
// ThemeSpec { material_style: LiquidGlass, .. }.theme().material_style == LiquidGlass; serde default is Solid;
// a DeriveSpec with Contrast::High yields reduce_transparency == true.
```

- [ ] **Step 2: 运行，确认失败** — `cargo test -p fenestra-core --lib surface`；预期：编译失败。
- [ ] **Step 3: 实现。** `Element::surface` 改成 `self.themed(move |t, s| t.apply_surface(role, s))`。`apply_surface` 在 Solid 下的行为和现在的 `Element::surface` 完全一样，包括 Flat 规则。
- [ ] **Step 4: 运行，确认通过** — `cargo test -p fenestra-core --lib && cargo test -p fenestra-kit`；预期：全部通过（kit 的截图基准不变）。
- [ ] **Step 5: 提交** — `git commit -m "feat(core): MaterialStyle in the theme, Bar/Dock/Window surfaces, one surface resolver"`

### Task 2: 各风格的配方、降低透明度、`label_on`

**Files:**
- Modify: `fenestra-core/src/surface.rs`、`theme.rs`
- Test: `fenestra-core/src/surface.rs` 的测试模块

**Interfaces:**
- Consumes：Task 1。
- Produces：`Theme::label_on(&self, role: Surface, level: Vibrancy) -> Color`。

**配方（默认缩放下的值）。** 玻璃用 `Material::new(fill, blur, saturation).tint(fill_role_color)`，再加 `backdrop_blur` 和 `backdrop_saturate`。数值和 wechat-ui 现在的 `native/src/fx.rs` 里 `liquid`、`acrylic`、`tonal` 函数一致，实现时以那里为准：

| 角色 | LiquidGlass | Acrylic | Tonal（`surface` 混入主题色） |
|---|---|---|---|
| Bar | 0.46 / 22 / 1.7，强边缘高光（0.5 / 0.14），自适应明暗，无阴影 | 0.78 / 30 / 0.95，噪点 0.02，无描边、无阴影 | 8% |
| Dock | 0.54 / 20 / 1.7，同上，阴影 Lg，`R_FULL` | 0.78 / 30 / 0.95，噪点 0.02，1px 细描边，阴影 Sm，`R_SM` | 11%，`R_FULL` |
| Popover、Menu | 0.64 / 20 / 1.7，阴影 Lg，`R_XL` | 同 Dock，`R_SM` | 12%，阴影 Md，`R_LG` |
| Glass | 0.5 / 16 / 1.7，阴影 Lg，`R_XL`，保留高光扫过 | 同 Dock，`R_SM` | 14%，阴影 Md，`R_XL` |
| Modal | 0.72 / 20 / 1.7，阴影 Xl，`R_XL` | 不透明的 `Elevated(2)`，阴影 Xl，`R_XL` | 14%，阴影 Xl，`R_XL` |
| Card、Raised | 同 Solid | 同 Solid | 5%，`R_LG`（Card 阴影 Sm） |
| Window | `bg` | `bg` 混入主题色 5%，表面噪点 0.02 | `bg` |
| Thumb、Tooltip | 同 Solid | 同 Solid | 同 Solid |

`reduce_transparency`：玻璃角色改用"同一个填充角色的不透明颜色"，去掉模糊、饱和度、噪点、高光扫过、边缘高光和自适应明暗；圆角、描边、阴影保留。Solid 的 `Glass` 也适用。

- [ ] **Step 1: 写失败的测试**

```rust
#[test] fn liquid_glass_frosts_only_what_floats()
// LiquidGlass: Bar/Dock/Popover/Menu/Glass/Modal have backdrop_blur Some and adaptive_tint Some; Card/Raised/Window have none.
#[test] fn acrylic_is_grained_thick_glass_over_mica()
// Acrylic: Popover has glass_noise 0.02, a 1px border, backdrop_blur 30; Bar has no border;
// Window fill == mix(bg, accent, 0.05) and surface_noise 0.02.
#[test] fn tonal_has_no_glass_and_rises_toward_the_accent()
// Tonal: no role has backdrop_blur; the accent share of the fill grows Card < Bar < Dock < Popover < Modal
// (e.g. measured as OKLab distance from theme.surface).
#[test] fn recipes_follow_the_radius_scale()
// with RadiusScale::sharp(), LiquidGlass Popover's radius == RadiusScale::sharp()'s R_XL-level value (smaller than the default).
#[test] fn flat_elevation_holds_in_every_style()
// for each MaterialStyle with Elevation::Flat: Card's shadow_token is None.
#[test] fn reduced_transparency_drops_vibrant_labels()
// LiquidGlass + reduce_transparency: no role has backdrop_blur; every glass role's fill alpha == 1;
// label_on(Bar, Primary) == theme.text. Without it: label_on(Bar, Primary) == theme.vibrant_label(Primary),
// label_on(Card, Primary) == theme.text, and Acrylic/Tonal/Solid label_on(Bar, Primary) == theme.text.
#[test] fn recipes_resolve_in_light_and_dark()
// Theme::light() LiquidGlass label_on(Bar, Primary) has black ink; Acrylic Window over light bg is lighter than over dark bg.
```

- [ ] **Step 2: 运行，确认失败** — `cargo test -p fenestra-core --lib surface`；预期：失败（目前只有 Solid 的配方）。
- [ ] **Step 3: 实现**：配方表、`reduce_transparency`、`label_on`。
- [ ] **Step 4: 运行，确认通过** — `cargo test -p fenestra-core --lib && cargo test -p fenestra-kit`；预期：全部通过。
- [ ] **Step 5: 提交** — `git commit -m "feat(core): Liquid Glass, Acrylic and Tonal surface recipes; reduce transparency; label_on"`

### Task 3: kit 组件改走角色

**Files:**
- Modify：`fenestra-kit/src/widgets/` 下所有自己拼浮层、菜单、弹窗、玻璃、工具栏样式的组件（盘点结果写进提交说明）。`toolbar` 改用 `Surface::Bar`；`glass_surface`、`glass_panel` 用 `Surface::Glass`。
- Test：`fenestra-kit/tests/material_style_roles.rs`（新建）

**Interfaces:**
- Consumes：Task 1 的 `Theme::apply_surface`、`MaterialStyle::ALL`。

- [ ] **Step 1: 写失败的测试**

```rust
#[test] fn kit_surfaces_follow_the_material_style()
// For each MaterialStyle: build a frame (Harness or build_frame) holding a menu (open), a popover (open), a select's open
// listbox, a toolbar and a glass_panel, with ids; for each, the frame's glass body (Frame::glass_body) is Some exactly when
// theme.with_material_style(style).apply_surface(role, Style::default()).backdrop_blur is Some, and for blurred roles
// GlassBody.tint matches that resolved fill.
```

- [ ] **Step 2: 运行，确认失败** — `cargo test -p fenestra-kit --test material_style_roles`；预期：toolbar 等没走角色的组件失败。
- [ ] **Step 3: 实现**：盘点并改造。
- [ ] **Step 4: 运行，确认通过** — `cargo test -p fenestra-kit`；预期：全部通过，现有截图基准不变。
- [ ] **Step 5: 提交** — `git commit -m "feat(kit): every floating and bar surface resolves through the theme's material style"`

### Task 4: `app_bar`、`tab_bar`、`scaffold`

**Files:**
- Create：`fenestra-kit/src/widgets/scaffold.rs`（在 `widgets/mod.rs` 和 `lib.rs` 里导出）
- Test：`fenestra-kit/tests/scaffold_test.rs`

**Interfaces:**
- Consumes：Task 1 和 Task 2。
- Produces（供 Task 7 使用）：
  - `pub fn app_bar<Msg>(title: impl Into<String>) -> AppBar<Msg>`：`.leading(Element)`、`.trailing(Element)`、`.id(&str)`，可转成 `Element`；`Surface::Bar`，高 48，标题文字色用 `label_on(Surface::Bar, Primary)`。
  - `pub fn tab_bar<Msg>(items: impl IntoIterator<Item = TabItem<Msg>>) -> Element<Msg>`，`pub fn tab_item<Msg>(label, icon: Element<Msg>, active: bool, on_select: Msg) -> TabItem<Msg>`；高 56，每一项是带名字的按钮（可访问性名称就是 label）。
  - `pub fn scaffold<Msg>(content: Element<Msg>) -> Scaffold<Msg>`：`.top(Element, height: f32)`、`.bottom(Element, height: f32)`、`.id(&str)`，可转成 `Element`。
    - 元素树结构固定：外层容器包含内容、底栏包装层和顶栏，底栏和顶栏绝对定位，后画所以在上层。
    - themed 闭包按 `t.material_style` 决定：
      - `bars_overlay_content()` 时，`content` 加上下内边距，内容从栏下面滚过；否则外层容器加内边距，内容夹在两栏之间；
      - `floating_dock()` 时，底栏离边：左右 `SP3`、底 `SP2`，`R_FULL` 胶囊形，使用 `Surface::Dock`；否则贴边，使用 `Surface::Bar`。

- [ ] **Step 1: 写失败的测试**

```rust
#[test] fn scaffold_lays_out_each_material_style()
// 411×800, a scaffold with app_bar("Title") top (48) and tab_bar(4 items) bottom (56), content a scroll col with id "list":
// LiquidGlass: list.y0 <= top.y0, list.y1 >= bottom.y1, bottom.x0 > 4 && bottom.x1 < 407;
// Acrylic: list under both bars, bottom spans 0..411; Solid and Tonal: list.y0 >= top.y1 && list.y1 <= bottom.y0.
#[test] fn switching_style_keeps_focus_and_scroll()
// Harness: focus a text input inside the scaffold content and scroll the list; rebuild with another MaterialStyle;
// the input keeps focus and the scroll offset is unchanged.
#[test] fn tab_bar_items_are_named_buttons()
// tab_item("Chats", …) is queryable as by::role(Button).name("Chats"); clicking it emits on_select.
```

- [ ] **Step 2: 运行，确认失败** — `cargo test -p fenestra-kit --test scaffold_test`；预期：编译失败。
- [ ] **Step 3: 实现。**
- [ ] **Step 4: 运行，确认通过** — `cargo test -p fenestra-kit`；预期：全部通过。
- [ ] **Step 5: 提交** — `git commit -m "feat(kit): app_bar, tab_bar and a scaffold that floats its bars by material style"`

### Task 5: looks 的默认风格，以及各风格的截图基准

**Files:**
- Modify：`fenestra-looks/src/lib.rs`（`Look::with_material_style(self, MaterialStyle) -> Look`，同时设置 `theme.material_style`）
- Create：`fenestra-kit/tests/material_styles_golden.rs`、它的截图（`tests/snapshots/material_style_{solid,liquid_glass,acrylic,tonal}.png`）
- Modify：`CHANGELOG.md`（`Theme` 的新字段、`Surface` 的新角色、kit 的新组件）

- [ ] **Step 1: 写测试**：
  - `look_material_style_sets_the_theme`：`product(Mode::Dark).with_material_style(LiquidGlass).theme.material_style == LiquidGlass`；六个 look 默认都是 `Solid`。
  - `material_style_goldens`：同一个场景，用 `Theme::dark()` 渲染四种风格各一张（640×420，`render_element` 加 `assert_png_snapshot`）。场景包括：条纹背景，上面一个 scaffold（app_bar、列表、tab_bar），外加打开的菜单和一块 glass_panel。
- [ ] **Step 2: 运行，确认失败** — `cargo test -p fenestra-looks && cargo test -p fenestra-kit --test material_styles_golden`；预期：编译失败，或者因为没有基准图而失败。
- [ ] **Step 3: 实现，并生成基准图**：`FENESTRA_UPDATE_SNAPSHOTS=1 cargo test -p fenestra-kit --test material_styles_golden`。逐张打开看：液态玻璃的栏是通透的玻璃，亚克力有磨砂颗粒和细描边，Tonal 没有模糊，Solid 的栏是实色。
- [ ] **Step 4: 运行，确认通过** — `cargo test -p fenestra-looks -p fenestra-kit`；预期：全部通过。
- [ ] **Step 5: 提交** — `git commit -m "feat(looks): a look can name its material style; goldens for the four styles"`

### Task 6: 全量检查、推送、CI

- [ ] **Step 1:** 跑上一份计划用过的本地检查：
  - `cargo fmt --all --check`；
  - `cargo clippy --workspace --all-targets -- -D warnings`；
  - `cargo test -p fenestra-core -p fenestra-raster -p fenestra-shell -p fenestra -p fenestra-kit -p fenestra-looks`；
  - 光栅化截图对比（33 个基线失败之外不能有新增）；
  - wasm、iOS、Android 编译检查，以及 game-activity 的 clippy。

  预期：全部通过。
- [ ] **Step 2:** 推送到 `fork/main`（SSH 不通时用 HTTPS 加 `gh auth git-credential`），一次推送完，然后 `gh workflow run CI --repo paulwang667/fenestra --ref main`。

### Task 7: wechat-ui 改用 fenestra 的实现

**Files:**
- Modify：`wechat-ui/native/src/ui.rs`、`mobile.rs`、`fx.rs`；`native/Cargo.lock`、`native/android/Cargo.lock`
- Test：`wechat-ui/native/tests/mobile.rs`

**Interfaces:**
- Consumes：Task 1、2、4。
- `UiStyle` 保留，作为 Model 里的设置值和显示名称，加上 `fn material(self) -> MaterialStyle`（Solid→Solid，Liquid→LiquidGlass，Acrylic→Acrylic，Material→Tonal）。`Model::theme()` 返回 `….with_material_style(self.style.material())`。

- [ ] **Step 1: 写测试**（加到 `tests/mobile.rs`）：
  - `the_theme_carries_the_style`：每种 `UiStyle` 下，`Model::theme().material_style == style.material()`。
- [ ] **Step 2: 运行，确认失败**：`cargo update -p fenestra`（`native/` 和 `native/android/`）之后运行 `cargo test --test mobile`；预期：编译失败。
- [ ] **Step 3: 实现**：
  - 栏、悬浮栏、浮层、通话卡片改用 `.surface(Surface::Bar | Dock | Menu | Popover | Glass)`。按设计，Solid 下的菜单、表情面板和通话卡片用 `Surface::Glass`。
  - 根元素用 `.surface(Surface::Window)`。
  - 栏上的文字用 `t.label_on(Surface::Bar, level)`。
  - 布局判断改用 `m.style.material().bars_overlay_content()` 和 `floating_dock()`；手机首页能用 `scaffold` 的就用。
  - 删掉 `fx.rs` 里的 `material`、`Layer`、`liquid`、`acrylic`、`tonal`、`window_base`、`bar_label`、`mix`、`tone`。聊天壁纸、aurora 和它们的表面噪点属于应用内容，保留。
- [ ] **Step 4: 运行，确认通过** — `cargo test`（`native/`）；预期：全部通过（原有 19 个手机测试加新测试）。`grep -n "fn liquid\|fn acrylic\|fn tonal\|fn material" src/fx.rs` 应该没有输出。
- [ ] **Step 5: 提交**（只在本地）— `git commit -m "refactor(native): the styles come from fenestra's material styles"`

### Task 8: 实测和截图对比

- [ ] Mac（release，`WECHATUI_GLASS=0/1/2/3`）和一加 6（`debug.wechatui.glass=0/1/2/3`）：截图，和迁移前（笔记目录的 `named-style-compare4-*.png`）对比；记录一加上聊天页滚动的帧间隔中位数，预期约 16.7ms。数据写进笔记目录的 `FRAME-TIME-31MS.md`。差异只允许出现在设计写明的地方（Solid 下浮层改用 `Glass` 角色），其他地方的差别要么修掉，要么在账本里写明。
