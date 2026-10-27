# 触摸滚动的惯性和边界回弹 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 手机上的触摸滚动有惯性、可以接住、到边界有橡皮筋和回弹；桌面行为不变。

**Architecture:** shell 的 `touch.rs` 把手指变成 `Pan` / `PanEnd` / `PanStop` 事件（抬手时附带速度）；core 的 `FrameState` 在每个滚动条目里存速度和越界量，建帧时按时间推进物理；逻辑偏移始终在范围内，越界量只作用于绘制和命中。

**Tech Stack:** Rust，fenestra-core、fenestra-shell（winit），无头 `Harness` 测试。

**Spec:** `docs/superpowers/specs/2026-10-27-touch-momentum-design.md`

## 全局约束

- 桌面、滚轮、触控板和无头渲染的行为与像素不变；现有测试不改断言（`touch.rs` 里断言触摸拖动发 `Wheel` 的测试改为断言 `Pan`）。
- 参数：速度窗口 100ms；抬手前最后一次移动早于 50ms 则速度为 0；速度上限 8000；起滑门槛 50；τ = 0.325s；停止速度 20；橡皮筋 (1 − 1/(x·0.55/d + 1))·d；回弹临界阻尼 ω = 12/s；冲出量 ≤ 视口 1/4（单位都是逻辑像素、秒）。
- `scroll_offset()` / `scroll_offset_x()`、滚动条、贴底判断只看逻辑偏移（0..=max）。
- `reduced_motion` 下没有越界（拖过头、撞边硬停），惯性保留。
- 运动停止后不再置 `animating`。
- 共享 `CARGO_TARGET_DIR`；不跑 `cargo test --workspace`；小改动按影响范围检查，推送前跑一次全量门控。

## 审查重点

1. 滑行中点滚动容器之外的按钮：照常点击（Task 3 测试 `a_touch_outside_the_moving_scroller_still_clicks`）。
2. 贴底的消息流：拖过底部再松手，回弹后仍贴底，新内容出现时跟随（Task 2 测试 `a_sticky_feed_stays_pinned_after_a_bounce`）。
3. 回弹中接住再拖：越界量从当前位置接着算，不跳（Task 1 测试 `catching_a_bounce_holds_it_and_drag_continues`）。
4. 滚动内容在滑行中消失（容器从树里移除）：不崩溃、不留下永远 `animating`（Task 2 测试 `a_vanished_scroller_stops_animating`）。
5. 嵌套：横向容器套在纵向容器里，斜着划只让各轴自己的容器滑行（Task 1 测试 `each_axis_flings_its_own_scroller`）。

---

### Task 1: core 的事件和滚动物理

**Files:**
- Modify: `fenestra-core/src/events.rs`（`InputEvent` 新变体及处理）、`fenestra-core/src/frame_state.rs`（`Scroll` 扩展、物理推进、查询）、`fenestra-core/src/lib.rs`（导出查询函数）
- Test: `fenestra-core/tests/touch_momentum.rs`（新建，用 `build_frame` 和共享 `FrameState`，或 kit/shell 的 Harness 若 core 测试不便）

**Interfaces:**
- Produces:
  - `InputEvent::Pan { dx: f32, dy: f32 }`：手指移动，内容跟手（core 内部按 `-dy` 滚动，与 `Wheel` 方向一致）。
  - `InputEvent::PanEnd { vx: f32, vy: f32 }`：抬手，速度为手指速度（逻辑像素每秒，内容随手指方向继续走）。
  - `InputEvent::PanStop`：在指针位置接住。
  - `pub fn scroll_moving_at(frame: &Frame, state: &FrameState, point: Point) -> bool`（`events.rs`，与 `owns_touch_drag` 并列导出）：落点各轴最近的滚动容器有速度或越界量。
  - `FrameState` 内部：`Scroll` 新增 `velocity: (f32, f32)`、`stretch: (f32, f32)`（显示越界量，`stretch_y < 0` 表示顶部被拉开）、`raw_overscroll: (f32, f32)`（拖动中的原始越界量）、`max: (f32, f32)`、`viewport: (f32, f32)`、`dragging: bool`、`last_step: f64`。
  - `pub(crate) fn advance_scroll(&mut self, id, now) -> bool`（返回是否仍在运动），供 Task 2 在建帧时调用；`pub(crate) fn scroll_stretch(&self, id) -> (f32, f32)`。

- [ ] **Step 1: 写失败的测试**（`fenestra-core/tests/touch_momentum.rs`，一个 400 高视口、40 行×48 的纵向列表，必要时一个横向列表）：
  - `a_fling_glides_and_slows_to_a_stop`：`Pan` 几次后 `PanEnd{vy:-2000}`（向上划，内容向下翻），推进 100ms 偏移增加，推进 2s 后偏移不再变化、`frame.animating == false`；总滑行距离 ≈ 2000·0.325（误差 10%）。
  - `a_slow_release_does_not_glide`：`PanEnd{vy:-40}` 后偏移不变。
  - `dragging_past_the_top_follows_the_rubber_band`：偏移 0 时 `Pan{dy:+100}`，`scroll_offset()==0`，绘制位移 = 橡皮筋公式（d = 视口高）。
  - `a_release_past_the_edge_springs_back`：越界后 `PanEnd{0,0}`，0.6s 后越界量 < 0.5、不再 animating。
  - `a_fling_into_the_end_overshoots_then_settles`：靠近底部时大速度 `PanEnd`，期间越界量 > 0 且 ≤ 视口/4，最终偏移 == max、越界量 0。
  - `pan_stop_halts_a_fling`：滑行中 `PanStop`，之后推进 500ms 偏移不变。
  - `catching_a_bounce_holds_it_and_drag_continues`：越界回弹中 `PanStop`，推进 200ms 越界量不变；再 `Pan` 同方向 10px，越界量单调变大且第一帧跳变 < 2px。
  - `reduced_motion_never_overscrolls`：`state.reduced_motion = true` 时拖过头和撞边越界量始终 0，惯性仍有。
  - `the_wheel_is_unchanged`：`Wheel` 超出范围后偏移被夹住、无越界、无速度。
  - `each_axis_flings_its_own_scroller`：横向套纵向，`PanEnd{vx, vy}` 两个容器各自滑行。
  - `scroll_moving_at_sees_only_the_moving_scroller`：滑行中，列表内的点为真，列表外的点为假。
- [ ] **Step 2: 运行，确认失败**：`cargo test -p fenestra-core --test touch_momentum` → 编译失败（`InputEvent::Pan` 不存在）。
- [ ] **Step 3: 实现**：
  - `Pan` 与 `Wheel` 共用 `on_wheel` 优先处理（`WheelEvent{dx,dy}`），之后各轴路由到 `scrollable_*_at`；在范围内直接改逻辑偏移，超出的部分进 `raw_overscroll` 并按公式得出 `stretch`（reduced_motion 时丢弃超出部分）；置 `dragging`、清速度。反向拖动先吃掉越界量再动偏移。
  - `PanEnd`：对指针处各轴容器：`dragging = false`；越界时速度交给弹簧；否则 |v| ≥ 50 时设 `velocity = -v`（偏移方向），截断到 8000。
  - `PanStop`：指针处各轴容器速度清零、`dragging = true`（弹簧暂停）；接着的 `Pan` 从当前 `stretch` 反解 `raw_overscroll`。
  - `advance_scroll`：dt = now − last_step（首帧 0，封顶 0.1s 防止卡顿后大跳）；惯性：位移 = v·τ·(1 − e^(−dt/τ))，v *= e^(−dt/τ)，|v| < 20 停；撞边时剩余速度（截断使冲出 ≤ 视口/4）交给弹簧、偏移夹在边缘；弹簧：临界阻尼解析解推进 stretch 与其速度，|stretch| < 0.5 且速度小时归零；`dragging` 时不推进弹簧。
- [ ] **Step 4: 运行，确认通过**：`cargo test -p fenestra-core --test touch_momentum`；预期全部通过。`cargo test -p fenestra-core` 全部通过。
- [ ] **Step 5: 提交**：`git commit -m "feat(core): touch pans fling and bounce at the edges"`

### Task 2: 建帧里的推进、绘制和贴底

**Files:**
- Modify: `fenestra-core/src/frame.rs`（滚动解析处：调用 `advance_scroll`、`child_origin` 加上 `stretch`、`animating`）、`fenestra-core/src/frame_state.rs`（`clamp_scroll_2d` 记录 `max`/`viewport`，`gc_scroll` 清理）
- Test: `fenestra-core/tests/touch_momentum.rs`

**Interfaces:**
- Consumes: Task 1 的 `advance_scroll`、`scroll_stretch`。

- [ ] **Step 1: 写失败的测试**：
  - `the_stretch_moves_what_is_painted_and_hit`：顶部拉开 60px 时，第一行的 `rect.y0` 比静止时大约 stretch，点在新位置命中第一行。
  - `a_sticky_feed_stays_pinned_after_a_bounce`：`stick_bottom` 的容器拖过底部再松手，回弹结束后 `at_bottom` 仍为真；追加内容后偏移跟到新底部。
  - `a_vanished_scroller_stops_animating`：滑行中把列表移出树，下一帧 `animating == false`，再下一帧仍为假。
  - `a_still_frame_does_not_animate`：从不触摸时 `animating` 与之前一致（只看滚动条淡出）。
- [ ] **Step 2: 运行，确认失败**。
- [ ] **Step 3: 实现**：在 `clamp_scroll_2d` 前调用 `advance_scroll(node.id, now)`，运动中 `self.animating = true`；`child_origin` 减去 `offset + stretch`；`clamp_scroll_2d` 存 `max` 和视口尺寸；越界时 `at_bottom` 按逻辑偏移判断（不受 stretch 影响）。
- [ ] **Step 4: 运行，确认通过**：`cargo test -p fenestra-core`；`cargo test -p fenestra-kit -p fenestra`（确认无回归）。
- [ ] **Step 5: 提交**：`git commit -m "feat(core): the frame advances and paints the scroll momentum"`

### Task 3: shell 的手指：Pan、速度、接住

**Files:**
- Modify: `fenestra-shell/src/touch.rs`、`fenestra-shell/src/window.rs`（主窗口和副窗口的 `WindowEvent::Touch` 分支传入"落点在动"的判断）、`fenestra-shell/src/harness.rs`（如需要，加 `fling(q, vx, vy)` 便捷方法——可选）
- Test: `fenestra-shell/src/touch.rs` 内的单元测试

**Interfaces:**
- Consumes: `fenestra_core::scroll_moving_at`、三种新事件。
- Produces: `Touch::event(&mut self, id, phase, at, now, owns_drag: bool, moving: bool)`（`moving` 只在 `Started` 时读）。

- [ ] **Step 1: 写失败的测试**（`touch.rs`）：
  - 现有拖动测试的 `Wheel` 断言改为 `Pan`。
  - `a_quick_swipe_ends_with_its_velocity`：10ms 间隔移动 5 次共 100px 后抬手，`PanEnd` 的 vy ≈ 2000（±10%）。
  - `a_held_finger_releases_without_velocity`：移动后停 80ms 再抬手，`PanEnd{0,0}`。
  - `the_velocity_is_capped`：极快移动，|v| ≤ 8000。
  - `a_touch_on_a_moving_scroller_catches_it`：`moving = true` 时按下发 `PanStop`、无 `PointerDown`；不动就抬手只发 `PanEnd{0,0}`，无 `PointerUp`；不触发长按。
  - `a_catch_that_drags_scrolls`：接住后拖过阈值发 `Pan`。
  - `a_touch_outside_the_moving_scroller_still_clicks`：`moving = false` 时照常 `PointerDown`/`PointerUp`。
  - `a_tap_sends_no_pan_end`：普通点击不发 `PanEnd`。
- [ ] **Step 2: 运行，确认失败**：`cargo test -p fenestra-shell --lib touch`。
- [ ] **Step 3: 实现**：`Mode::Caught`；样本环形缓冲（最近 100ms）；拖动发 `scroll_step` 改为 `Pan`；`Scrolling` 和 `Caught` 抬手发 `PanEnd`；`Cancelled` 发 `PanEnd{0,0}`（让越界弹回）。`window.rs` 两处在 `Started` 时用 `self.last` 的 frame 算 `scroll_moving_at`。
- [ ] **Step 4: 运行，确认通过**：`cargo test -p fenestra-shell`。
- [ ] **Step 5: 提交**：`git commit -m "feat(shell): a lifted finger flings, a touch on a moving scroller catches it"`

### Task 4: 文档、全量检查、推送

- [ ] CHANGELOG：Added（惯性、接住、回弹，只在触摸平台）；Breaking（`InputEvent` 新增三个变体，外部穷尽匹配需要处理）。
- [ ] 全量门控（fmt、workspace clippy、core/raster/shell/fenestra/kit/looks 测试、光栅化截图对比无新增失败、wasm、iOS、Android、game-activity clippy）。
- [ ] 推送 fork/main，触发 CI。

### Task 5: 真机验证

- [ ] wechat-ui 更新 fenestra 依赖，`cargo test`（native）全部通过，本地提交。
- [ ] 一加 6：会话列表、聊天消息流、通讯录上快速划动、接住、拖过头、撞边；截图（含越界瞬间）；滑行时帧间隔中位数（预期约 16.7ms）；滑行中点底部标签能切换。数据写进笔记目录 `FRAME-TIME-31MS.md`。测完清掉 `debug.wechatui.*`，输入法仍是 Gboard。
