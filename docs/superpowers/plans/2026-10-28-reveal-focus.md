# 获得焦点的元素滚进可见区域 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 焦点变化或有焦点时窗口尺寸变化的那一帧，把焦点元素滚进各级滚动容器的可见区域。

**Architecture:** `FrameState` 记住上一帧的焦点与尺寸；`build_frame` 在布局和浮层之后、组装 `Frame` 之前，若需要就沿焦点元素的滚动祖先链（由内向外）用 `scroll_to` / `scroll_to_x` 调整偏移，并置 `animating`。

**Tech Stack:** Rust，fenestra-core。

**Spec:** `docs/superpowers/specs/2026-10-28-reveal-focus-design.md`

## 全局约束

- 只在焦点变化或（有焦点时）尺寸变化那一帧做一次。
- 已完整可见时不改偏移。
- 边距 `REVEAL_MARGIN = 8.0`；目标大于可见区域时对齐起始边。
- 可见区域是滚动容器的内容框（扣掉内边距）。

## 审查重点

1. 焦点不变时用户滚走，不被拉回（测试 `a_scrolled_away_field_is_left_alone`）。
2. 惯性滑行中焦点不变，不打断（同上测试覆盖：不调用 `scroll_to`）。
3. 浮动栏的内边距：元素不藏在内边距下（测试 `padding_counts_as_hidden`）。
4. 嵌套滚动容器（测试 `nested_scrollers_both_reveal`）。
5. 焦点元素在浮层里（测试 `a_focused_menu_item_is_revealed_in_its_overlay`）。

---

### Task 1: core 的滚进视野

**Files:** `fenestra-core/src/frame.rs`、`fenestra-core/src/frame_state.rs`；测试 `fenestra-core/tests/reveal_focus.rs`（新建）。

- [ ] 写失败的测试（400 宽 300 高窗口；一个 200 高、内含 20 个 48 高输入框的纵向滚动列表）：
  - `tab_into_a_field_below_the_fold_reveals_it`：Tab 到第 10 个输入框后再建一帧，它的矩形在列表内容框内（含 8 像素边距，允许 0.5 误差）。
  - `a_shorter_window_reveals_the_focused_field`：聚焦列表里靠下但可见的字段，用更矮的窗口（列表 `h_full`）重建 → 它仍在可见区域。
  - `a_visible_field_does_not_scroll`：聚焦第一个字段，偏移保持 0。
  - `a_scrolled_away_field_is_left_alone`：聚焦后 `scroll_by` 把它滚出视野，再建两帧，偏移不回来。
  - `nested_scrollers_both_reveal`：外层纵向列表里有一个内层纵向列表，内层第 15 个字段聚焦 → 内外都滚到可见。
  - `a_field_taller_than_the_view_aligns_its_top`：一个 300 高的文本框在 200 高列表的下方，聚焦 → 顶部对齐（含边距）。
  - `padding_counts_as_hidden`：列表 `pt(60)`（模拟浮动栏），聚焦一个被滚到顶部内边距下的字段 → 滚动后它在内边距之下。
  - `a_focused_menu_item_is_revealed_in_its_overlay`：打开的浮层里的滚动列表，焦点设到视野外的一项 → 浮层内滚动后可见。
- [ ] 运行确认失败：`cargo test -p fenestra-core --test reveal_focus`。
- [ ] 实现：`FrameState` 加 `revealed_focus: Option<WidgetId>`、`revealed_size: (f32, f32)`；`frame.rs` 加 `reveal(node, id, state)`：找祖先链，按 spec 算法逐层调整；在组装 `Frame` 前对主树和各浮层调用。
- [ ] 运行确认通过：`cargo test -p fenestra-core`、`-p fenestra-kit`、`-p fenestra`。
- [ ] 提交：`feat(core): the focused element scrolls into view`。

### Task 2: 文档、门控、推送、真机

- [ ] CHANGELOG（Added）。全量门控；推送；CI。
- [ ] wechat-ui 更新依赖，`cargo test` 通过，本地提交。
- [ ] 一加 6：临时探针页（长列表 + 底部输入框），点击输入框、键盘弹出后截图，输入框在可见区域；清理调试属性。
