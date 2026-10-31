//! System clipboard on Android. Freya's clipboard backend (copypasta) is
//! a no-op there, so copy and paste go through `ClipboardManager` over
//! JNI instead.

use freya::prelude::GlobalContexts;
use freya_clipboard::copypasta::ClipboardProvider;
use freya_clipboard::prelude::Clipboard;
use jni::JavaVM;
use jni::objects::{JObject, JString, JValue};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Replace Freya's clipboard with the system one. Call from inside the
/// app, once the launch has installed its own.
#[sismo::function("clipboard::install")]
pub fn install() {
    GlobalContexts::get().insert_context(Clipboard::create(Some(Box::new(AndroidClipboard))));
}

struct AndroidClipboard;

impl ClipboardProvider for AndroidClipboard {
    #[sismo::function("clipboard::get_contents")]
    fn get_contents(&mut self) -> Result<String, Error> {
        with_manager(|env, manager| {
            let clip = env
                .call_method(
                    manager,
                    "getPrimaryClip",
                    "()Landroid/content/ClipData;",
                    &[],
                )?
                .l()?;
            if clip.is_null() {
                return Ok(String::new());
            }
            let item = env
                .call_method(
                    &clip,
                    "getItemAt",
                    "(I)Landroid/content/ClipData$Item;",
                    &[JValue::Int(0)],
                )?
                .l()?;
            let text = env
                .call_method(&item, "getText", "()Ljava/lang/CharSequence;", &[])?
                .l()?;
            if text.is_null() {
                return Ok(String::new());
            }
            let text = env
                .call_method(&text, "toString", "()Ljava/lang/String;", &[])?
                .l()?;
            Ok(env.get_string(&JString::from(text))?.into())
        })
    }

    #[sismo::function("clipboard::set_contents")]
    fn set_contents(&mut self, contents: String) -> Result<(), Error> {
        with_manager(|env, manager| {
            let label = env.new_string("notein")?;
            let text = env.new_string(contents)?;
            let clip = env
                .call_static_method(
                    "android/content/ClipData",
                    "newPlainText",
                    "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                    &[JValue::Object(&label), JValue::Object(&text)],
                )?
                .l()?;
            env.call_method(
                manager,
                "setPrimaryClip",
                "(Landroid/content/ClipData;)V",
                &[JValue::Object(&clip)],
            )?;
            Ok(())
        })
    }
}

fn with_manager<T>(
    run: impl FnOnce(&mut jni::JNIEnv<'_>, &JObject<'_>) -> Result<T, Error>,
) -> Result<T, Error> {
    let app = istmo::android::android_app().ok_or("android app not set")?;
    // SAFETY: both pointers come from the running `AndroidApp` and stay
    // valid for the life of the activity.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast())? };
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let result = manager(&mut env, &activity).and_then(|manager| run(&mut env, &manager));
    // A failed call leaves its Java exception pending on the thread.
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    result
}

fn manager<'local>(
    env: &mut jni::JNIEnv<'local>,
    activity: &JObject<'_>,
) -> Result<JObject<'local>, Error> {
    let service = env.new_string("clipboard")?;
    let manager = env
        .call_method(
            activity,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[JValue::Object(&service)],
        )?
        .l()?;
    if manager.is_null() {
        return Err("no clipboard service".into());
    }
    Ok(manager)
}
