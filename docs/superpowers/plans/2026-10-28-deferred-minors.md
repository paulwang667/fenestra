# 推迟的小问题清理 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 修掉触摸惯性、键盘回车键、焦点滚进视野、页面动画、触感反馈几轮审查中推迟的小问题（用户 2026-10-28 选择"清理推迟的小问题"）。

**Spec:** 各问题的原始设计：`docs/superpowers/specs/2026-10-27-touch-momentum-design.md`、`2026-10-28-keyboard-type-and-enter-key-design.md`、`2026-10-28-reveal-focus-design.md`、`2026-10-28-page-transitions-design.md`、`2026-10-28-haptics-design.md`。本计划只修已记录的缺陷，不改设计意图。

## 全局约束

- 每项先写会失败的测试再修（纯文档项除外）；每个任务结束跑受影响包的测试；推送前全量门控。
- 不在本批：退出快照按需生成、系统触摸时间戳、虚拟列表空白条、吸顶标题、wechat-ui 风格判断重构。

---

### Task 1: 焦点、回车键、kit

- [ ] 测试并修复：`Next` 在 roving 范围里按范围容器找下一项；找不到下一项时不吞掉回车（`took` 只在真的移动了焦点时置真）。测试 `next_inside_a_roving_scope_moves_on`、`next_that_cannot_move_leaves_enter_to_the_window`。
- [ ] 测试并修复：显式完成/搜索/前往失焦时同时清除 `focus_visible`；失焦或"下一项"之后不再对旧焦点执行 Enter/Space 的点击与浮层开关。测试 `an_enter_key_action_clears_focus_visible`、`an_enter_key_action_does_not_also_click`。
- [ ] 删除不再读取的 `EditorState::submit_on_enter`。
- [ ] kit：`on_submit` 只在不带修饰键的回车触发（测试 `on_submit_ignores_modified_enter`）；文档写明它会接住回车（因此"下一项"不跳、完成/搜索/前往不失焦），以及应用再加 `.on_key` 会覆盖它。

### Task 2: 焦点滚进视野

- [ ] 测试并修复：计算新偏移时计入当前越界量（`scroll_to(offset + stretch + d)`）。测试 `a_reveal_during_a_bounce_lands_fully_in_view`。
- [ ] 测试并修复：焦点元素不在树里时不记为"已滚进视野"，出现后再滚。测试 `a_focused_element_that_appears_later_is_revealed`。
- [ ] 测试并修复：`chain_to` 跳过 `display: none` 子树。测试 `a_hidden_focused_element_does_not_scroll`。
- [ ] 补测试：横向滚动容器 `a_field_in_a_horizontal_scroller_is_revealed`；贴底消息流 `revealing_in_a_sticky_feed_keeps_it_pinned_at_the_bottom`（滚回底部的字段聚焦后仍贴底）。

### Task 3: 触摸、动画、触感

- [ ] 触摸：落点在正在滑行的容器上时，即使元素自带拖动（输入框、滑块），也优先接住。测试 `a_touch_on_a_moving_scroller_catches_even_over_a_field`。
- [ ] 动画：键盘焦点或减少动态效果跳过动画时，仍保持动画条目存活，之后不重播进入动画。测试 `keyboard_focus_does_not_replay_an_entrance`。
- [ ] 文档：`.transition()` 会替换进入动画的属性开关（不只是时长）；`enter_from` 的缩放和透明度是绝对值、位移是叠加值。
- [ ] 触感：`touch_input_main` 不再克隆事件（先算出是不是长按）；wechat-ui 的 `play_haptic` 注释记下"在非 UI 线程调用"的决定。

### Task 4: 门控、推送、wechat-ui

- [ ] CHANGELOG（Fixed）；全量门控；推送；CI；wechat-ui 更新依赖与注释，`cargo test` 通过，本地提交。
- [ ] 独立审查与一轮修复。
