# 具名材质风格进入 fenestra：设计

日期：2026-10-27
仓库：fenestra（`/Users/xxxx/projects/fenestra`，推送到 fork `paulwang667/fenestra` 的 `main`）。迁移验证：wechat-ui（`/Users/xxxx/projects/wechat-ui/native`）。

## 目的
wechat-ui 里调出了四种具名风格：实色、液态玻璃（Apple）、亚克力（Fluent）、Material 色调（Google）。每种风格是一整套配方，按层级（栏、悬浮栏、浮层、卡片）定义材质厚度、模糊、饱和度、噪点、边缘高光、描边、圆角、阴影、背景和文字色。目前这些配方写在 wechat-ui 的 `fx.rs` 里，别的项目用不上。

这次把它们搬进 fenestra，做成主题的一部分：应用选一种风格，kit 的组件和应用里按语义标注的表面（`Surface` 角色）就自动换成那套外观。wechat-ui 改用 fenestra 的实现，删掉自己的配方，用来验证迁移是完整的。

## 目标
- **风格进主题**：fenestra-core 新增 `MaterialStyle { Solid, LiquidGlass, Acrylic, Tonal }`，作为 `Theme` 的字段，默认是 `Solid`。另外提供 `Theme::with_material_style(style)`。
- **配方集中在一处**：`Theme::surface_style(role)` 按主题的风格解析每个 `Surface` 角色，所有配方都定义在这一处，不散落在组件里。
- **补齐表面角色**：
  - `Surface::Bar`：顶栏、头部、底部导航，内容在它下面或旁边；
  - `Surface::Dock`：悬浮的控件条，例如输入栏、悬浮 tab 栏；
  - `Surface::Window`：窗口底色，即亚克力的 Mica 底，其他风格就是页面色。

  已有角色（`Card`、`Raised`、`Popover`、`Menu`、`Modal`、`Glass`、`Thumb`、`Tooltip`）在每种风格下也都有定义。
- **玻璃上的文字色**：提供 `Theme::label_on(role, Vibrancy)`。液态玻璃下，玻璃角色上的文字用活力色（`vibrant_label`）；其他风格和非玻璃角色返回普通的 `text`、`text_muted`、`text_subtle`。
- **kit 组件自动采用**：凡是表达"浮层、菜单、弹窗、玻璃、工具栏"的 kit 组件，都通过 `Surface` 角色取外观，换风格就跟着变，组件代码不用分支判断风格。
- **补齐布局组件**：kit 新增三个组件：
  - `app_bar`：顶栏；
  - `tab_bar`：底部导航；
  - `scaffold`：页面骨架，包含顶栏、内容、底栏或悬浮栏。

  在需要"内容从栏下面滚过"的风格（液态玻璃、亚克力）下，`scaffold` 自动把栏做成覆盖在内容上的层，并给内容补上对应的内边距；在液态玻璃下，底栏自动做成离开屏幕边缘的悬浮胶囊。应用不用自己写覆盖层。
- **降低透明度**：`Theme` 新增 `reduce_transparency: bool`。打开后，所有玻璃角色解析成同色系的不透明填充（Apple 和 Fluent 的无障碍做法），布局不变。
- **looks 可以指定默认风格**：`Look` 新增 `with_material_style(style)`，现有六个 look 的默认风格保持 `Solid`。
- **wechat-ui 迁移**：
  - 风格设置直接改 `Theme` 的风格（`UiStyle` 映射到 `MaterialStyle`，或者直接用 `MaterialStyle`）；
  - 所有栏、悬浮栏、浮层、卡片改用 `Surface` 角色；
  - 玻璃上的文字改用 `label_on`；
  - `fx.rs` 里的 `material`、`Layer` 和各风格的配方函数删掉；
  - 外观和性能与迁移前一致（见验收）。

## 非目标
- 不改 `Solid` 风格的现有外观：现有组件、截图基准逐像素不变。
- 不新增别的风格（例如 macOS vibrancy、Windows Mica 读桌面壁纸）；系统级窗口材质另立项。
- 不在 fenestra 里提供"聊天壁纸"之类的应用内容。液态玻璃最适合浮在丰富的背景上，但背景放什么由应用决定。可以在 kit 里提供一个可选的渐变壁纸辅助函数，不是必须的。
- 不做跟随设备运动的动态高光、按压时的液态形变等 Liquid Glass 动效。
- 不改渲染后端；需要的能力（模糊、饱和度、噪点、边缘高光、自适应明暗、折射、阴影）都已经有了。

## 反目标（技术上能跑，但算失败的情况）
- 默认风格下任何现有截图基准变化，或者现有应用不改代码外观就变了。
- 同一个角色在不同组件里长得不一样（又回到配方散落在组件里）。
- 应用为了用风格，不得不在自己的代码里写 `match theme.material_style`（个别布局决策除外，而且应该有 kit 组件可用）。
- 玻璃角色铺满整屏或叠在一起：例如液态玻璃下把列表、卡片也做成玻璃，或者玻璃浮层下面还是玻璃，违背三家的规范，在手机上也会掉帧。
- 打开降低透明度后，仍有半透明或模糊。
- wechat-ui 迁移后外观明显变化，或者一加 6 上聊天页滚动掉出满帧。

## 约束
- fenestra-core 的公开接口只能增加。给 `Theme`（公开字段）加字段，会让外部用结构体字面量构造 `Theme` 的代码编译不过。仓库里 `Theme` 都是通过构造函数生成的，接受这一点，并在 CHANGELOG 里写明。`Surface` 是 `#[non_exhaustive]`，加变体是兼容的。
- 现有的风格无关参数都要遵守：`RadiusScale`（圆角缩放，例如 `sharp` 主题会把所有圆角变小）、`Elevation::Flat`（静止卡片不要阴影）、`Contrast`。
- 玻璃的性能代价（见笔记 `FRAME-TIME-31MS.md`）：一加 6 上每块整屏玻璃约 3.8ms，一帧的玻璃 pass 共用。配方和 `scaffold` 不能导致整屏的玻璃，而且一屏里同时出现的玻璃块数要少。
- 整个工作区禁止 `unsafe`；必须能在 wasm32、Android、iOS 上编译；测试按包运行，不跑 `cargo test --workspace`。

## 已经决定的做法
**每种风格的角色配方。** 数值起点取自 wechat-ui 实测调好的版本（`fx.rs` 的 `liquid`、`acrylic`、`tonal` 函数），实现时可以微调。

- **Solid**：所有角色和现在完全一样。新角色的定义：
  - `Bar`、`Dock`：实色 `surface`，带细边；
  - `Window`：页面色 `bg`。
- **LiquidGlass**：
  - `Bar`：底色约 0.46、模糊 22、饱和度 1.7、明亮的边缘高光、自适应明暗，无阴影；
  - `Dock`：约 0.54、模糊 20、阴影 Lg、胶囊形圆角；
  - `Popover`、`Menu`：约 0.64、圆角 18、阴影 Lg；
  - `Glass`（浮起的玻璃卡片或面板，例如通话卡片）：约 0.5、模糊 16、圆角 22、阴影 Lg，保留高光扫过；
  - `Modal`：更厚（约 0.72）、圆角 22；
  - `Card`、`Raised`：保持实色，属于内容层，不用玻璃；
  - 玻璃角色上的文字用活力色。
- **Acrylic**：
  - 玻璃角色（`Bar`、`Dock`、`Popover`、`Menu`、`Glass`）：底色约 0.78、模糊 30、饱和度 0.95、噪点 0.02；`Bar` 不描边，其余 1px 细描边、阴影 Sm、圆角 4～6（`Glass` 为 6）；
  - `Modal`：不透明的抬升填充；
  - `Window`：主题色混入页面色约 5%，加 0.02 的表面噪点；
  - `Card`、`Raised`：实色。
- **Tonal**（Material 3）：
  - 不用玻璃，填充是 `surface` 混入主题色：`Card` 和 `Raised` 5%、`Bar` 8%、`Dock` 11%、`Popover` 和 `Menu` 12%、`Glass` 和 `Modal` 14%；
  - 只有浮起的角色有阴影（`Glass` 为 Md）；
  - 圆角偏大：`Card` 16、`Popover` 16、`Glass` 20、`Modal` 28、`Dock` 胶囊形。

**栏的布局行为由 `MaterialStyle` 提供，`scaffold` 来用：**
- `MaterialStyle::bars_overlay_content()`：液态玻璃和亚克力为 true；
- `MaterialStyle::floating_dock()`：只有液态玻璃为 true。

应用自己写布局时也能用这两个查询。

**wechat-ui 的风格设置名称保持不变**：实色、液态玻璃、亚克力、Material 色调。

以下几条是设计审查后由我（Claude）决定的，审读时请留意：
- **wechat-ui 的浮层在 Solid 下映射到 `Surface::Glass`**：wechat-ui 的默认外观本来就让菜单、弹出的表情面板和通话卡片带玻璃（"玻璃只用于浮在内容上的元素"），而 fenestra 的 Solid `Menu`/`Popover` 是不透明的。所以 wechat-ui 把这些元素标成 `Surface::Glass`，迁移后仍是玻璃。参数会和 wechat-ui 现在的两组预设（0.78 / 18、0.58 / 14）略有出入，这个差异可以接受，验收里写明。
- **通话卡片这类"浮起的玻璃卡片"用 `Glass` 角色**，不新增角色。`Glass` 在每种风格下的配方按上面写的定义，也就是 wechat-ui 现在通话卡片的样子。`Card` 继续是内容层里静止的卡片。
- **配方里的圆角都对应到已有的圆角级别**（`R_SM` 6、`R_MD` 10、`R_LG` 14、`R_XL` 20、`R_FULL`），通过主题的 `RadiusScale` 解析，和现有角色一样（`sharp` 主题会一起变小）。文中的像素值是默认缩放下的值：18、22 取最接近的级别并允许微调，不给 `RadiusScale` 加新字段。
- **Solid 下的 `Glass` 保持现状**（玻璃、高光扫过、边缘高光、自适应明暗）。"玻璃角色"指在当前风格下有模糊的角色，所以 Solid 下只有 `Glass`。`reduce_transparency` 对所有风格的玻璃角色都生效，包括 Solid 的 `Glass`；它默认关闭，所以不影响现有外观。活力文字色只在液态玻璃下使用。
- **`MaterialStyle` 也进 `ThemeSpec`**（serde 字段，默认 Solid），这样用 spec 生成的主题也能指定风格。`Contrast::High` 时自动视为 `reduce_transparency`，三家都把"增强对比度"和"降低透明度"放在一起处理。

## 留给实现者决定
- `MaterialStyle` 放在哪个模块（`surface.rs` 或单独一个文件），配方表示成每种风格一张表，还是每个角色一个 match。
- `app_bar`、`tab_bar`、`scaffold` 的具体接口（建造者的方法名、插槽），只要能覆盖 wechat-ui 手机首页和聊天页的用法。
- kit 里哪些组件需要改成走角色：盘点时把所有直接拼"浮层或玻璃样式"的地方都改掉。
- 可选的渐变壁纸辅助函数是否提供、放在哪里。
- 各风格新截图基准的场景选择（建议用 kit 的 demo 页）。

## 验收
- **新测试（fenestra）**：
  - 每种风格下，每个角色解析出的样式符合配方，例如：液态玻璃的 `Bar` 有模糊、自适应明暗、边缘高光，`Card` 没有模糊；亚克力的 `Popover` 有噪点和 1px 描边；Tonal 的所有角色都没有模糊，填充按层级越来越偏向主题色。
  - 默认风格下，`surface_style` 对所有已有角色的输出和现在完全相同。
  - 打开降低透明度后，任何角色都没有模糊、填充不透明。
  - `label_on` 只在液态玻璃的玻璃角色上返回活力色。
  - `scaffold` 在液态玻璃和亚克力下把内容延伸到栏下面，在液态玻璃下底栏悬浮离边；Solid 和 Tonal 下所有元素都在正常排布中。
  - kit 组件（菜单、弹窗、选择框、工具栏）在每种风格下取到对应角色的样式。
- **截图基准**：每种风格新增一组 kit demo 截图（vello）；现有截图基准全部不变。
- **原有检查**：fenestra 全部测试、clippy（桌面和 Android）、wasm 和 iOS 编译检查都通过；推送后 CI 全绿。
- **wechat-ui**：
  - `fx.rs` 里不再有风格配方；
  - 全部测试通过；
  - 四种风格在 Mac 和一加 6 上截图，和迁移前对比看不出明显差别。Solid 下浮层的玻璃参数改用 fenestra 的 `Glass` 角色，允许有细微差别；
  - 一加 6 上聊天页滚动保持约 16.7ms。
