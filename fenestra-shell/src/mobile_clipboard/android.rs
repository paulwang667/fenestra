//! `android.content.ClipboardManager`, reached from the activity.

use jni::objects::{JObject, JString};
use jni::{Env, JValue, jni_sig, jni_str};
use winit::platform::android::activity::AndroidApp;

/// The system clipboard. Every call attaches the event-loop thread to the
/// JVM and looks the service up again: clipboard use is rare and the
/// lookup is cached by the activity.
pub(crate) struct AndroidClipboard {
    app: AndroidApp,
}

impl AndroidClipboard {
    pub(crate) fn new(app: AndroidApp) -> Self {
        Self { app }
    }

    /// Runs `f` with the clipboard service; a Java exception is cleared
    /// and comes back as `None`.
    fn with_manager<R>(
        &self,
        f: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> jni::errors::Result<R>,
    ) -> Option<R> {
        // SAFETY: android-activity hands out the process's JavaVM and a
        // global reference to the activity, both valid while the app runs;
        // neither wrapper frees what it wraps.
        let vm = unsafe { jni::JavaVM::from_raw(self.app.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env| -> jni::errors::Result<Option<R>> {
            let activity = unsafe { JObject::from_raw(env, self.app.activity_as_ptr().cast()) };
            let result = (|| {
                let name = env.new_string("clipboard")?;
                let manager = env
                    .call_method(
                        &activity,
                        jni_str!("getSystemService"),
                        jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
                        &[JValue::Object(&name)],
                    )?
                    .l()?;
                if manager.is_null() {
                    return Ok(None);
                }
                f(env, &manager).map(Some)
            })();
            if env.exception_check() {
                env.exception_clear();
            }
            Ok(result.ok().flatten())
        })
        .ok()
        .flatten()
    }
}

impl fenestra_core::Clipboard for AndroidClipboard {
    fn get(&mut self) -> Option<String> {
        let app = self.app.clone();
        self.with_manager(|env, manager| {
            let clip = env
                .call_method(
                    manager,
                    jni_str!("getPrimaryClip"),
                    jni_sig!("()Landroid/content/ClipData;"),
                    &[],
                )?
                .l()?;
            if clip.is_null() {
                return Ok(None);
            }
            let count = env
                .call_method(&clip, jni_str!("getItemCount"), jni_sig!("()I"), &[])?
                .i()?;
            if count < 1 {
                return Ok(None);
            }
            let item = env
                .call_method(
                    &clip,
                    jni_str!("getItemAt"),
                    jni_sig!("(I)Landroid/content/ClipData$Item;"),
                    &[JValue::Int(0)],
                )?
                .l()?;
            // SAFETY: as in `with_manager`.
            let activity = unsafe { JObject::from_raw(env, app.activity_as_ptr().cast()) };
            let text = env
                .call_method(
                    &item,
                    jni_str!("coerceToText"),
                    jni_sig!("(Landroid/content/Context;)Ljava/lang/CharSequence;"),
                    &[JValue::Object(&activity)],
                )?
                .l()?;
            if text.is_null() {
                return Ok(None);
            }
            let string = env
                .call_method(
                    &text,
                    jni_str!("toString"),
                    jni_sig!("()Ljava/lang/String;"),
                    &[],
                )?
                .l()?;
            let string: JString = env.cast_local::<JString>(string)?;
            Ok(Some(string.try_to_string(env)?))
        })
        .flatten()
    }

    fn set(&mut self, text: String) {
        self.with_manager(|env, manager| {
            let label = env.new_string("text")?;
            let text = env.new_string(text)?;
            let clip = env
                .call_static_method(
                    jni_str!("android/content/ClipData"),
                    jni_str!("newPlainText"),
                    jni_sig!(
                        "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;"
                    ),
                    &[JValue::Object(&label), JValue::Object(&text)],
                )?
                .l()?;
            env.call_method(
                manager,
                jni_str!("setPrimaryClip"),
                jni_sig!("(Landroid/content/ClipData;)V"),
                &[JValue::Object(&clip)],
            )?;
            Ok(())
        });
    }

    fn has_text(&mut self) -> bool {
        self.with_manager(|env, manager| {
            env.call_method(manager, jni_str!("hasPrimaryClip"), jni_sig!("()Z"), &[])?
                .z()
        })
        .unwrap_or(false)
    }
}
