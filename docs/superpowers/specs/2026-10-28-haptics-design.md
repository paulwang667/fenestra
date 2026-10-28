# 长按的触感反馈 设计

## 为什么做

在手机上长按会话弹出菜单时，手上没有原生应用那"咔"的一下，用户不确定长按是否生效。用户在 2026-10-28 让我做下一项（"Do haptics"），流程由我自行推进。

## 目标

1. 触摸长按**确实触发了动作**时（长按变成的右键产生了应用消息，比如打开上下文菜单），手机振动一下（系统的"长按"触感）。
2. 长按空白处、什么都没发生时不振动。
3. 遵守系统的"触摸振动"设置（关掉就不振）。

## 不做什么

- iOS（需要 Objective-C 接入，以后再做）。
- 让应用主动请求任意触感（点击、开关切换等）。`Haptic` 枚举留好扩展位。
- 桌面。

## 怎么做

- fenestra 不写 `unsafe`、没有 JNI 依赖，平台调用一律由应用提供，沿用 `with_android_back` 的模式：
  - core：`#[non_exhaustive] pub enum Haptic { LongPress }`。
  - shell：`WindowOptions::with_android_haptic(fn(&AndroidApp, Haptic))`。长按到期时，shell 照常派发右键事件；若右键产生了应用消息，就调用这个钩子。
- wechat-ui 的 Android 入口用 JNI 实现钩子：`activity.getWindow().getDecorView().performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)`。这个接口不需要振动权限，并且自动遵守系统的触摸振动设置。

## 怎么验证

- shell：判定"长按是否该振动"的逻辑抽成一个可在主机上测试的函数（输入：长按派发是否产生消息；输出：要不要振）。
- 真机（一加 6）：长按会话后 `adb shell dumpsys vibrator` 里出现一次新的振动记录；长按空白处没有。

## 我替用户做的决定

- 只在长按产生应用消息时振动。
- 用系统的"长按"触感常量，不自定义时长和强度。
