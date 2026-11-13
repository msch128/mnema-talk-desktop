//! The certificate check on Android is the system's own trust manager, which
//! `rustls-platform-verifier` reaches through JNI. It needs the JVM and the
//! app's context once, before the first connection; the window system has
//! recorded both in `ndk-context` by the time the app is set up. Its Kotlin
//! half comes into the APK from the Gradle build (gen/android).
use jni::{JavaVM, objects::JObject};

pub fn init() {
    if let Err(why) = try_init() {
        // Without it every HTTPS connection fails its certificate check, and
        // the picker says so; nothing is ever accepted unchecked.
        eprintln!("Mnema Talk: the Android certificate check could not start: {why}");
    }
}

fn try_init() -> Result<(), jni::errors::Error> {
    let context = ndk_context::android_context();
    // SAFETY: the pointers come from the Android runtime through ndk-context
    // and stay valid for the life of the process.
    let vm = unsafe { JavaVM::from_raw(context.vm().cast()) };
    vm.attach_current_thread(|env| {
        // SAFETY: a live reference to the activity, owned by the runtime.
        let activity = unsafe { JObject::from_raw(env, context.context().cast()) };
        rustls_platform_verifier::android::init_with_env(env, activity)
    })
}
