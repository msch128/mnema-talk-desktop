// The Kotlin half of rustls-platform-verifier (mobile/src/android_tls.rs): the
// Rust side asks the system trust manager through it. Taken from the crate's
// Maven archive at a fixed commit; the version must equal
// rustls-platform-verifier-android in mobile/Cargo.lock. Applied from
// gen/android/app/build.gradle.kts.
repositories {
    maven {
        url = uri("https://raw.githubusercontent.com/rustls/rustls-platform-verifier/1aa691352a5e0c215210dfc9a3c2f2078639dc91/android-release-support/maven/")
        content { includeGroup("org.rustls") }
    }
}

dependencies {
    "implementation"("org.rustls:rustls-platform-verifier:0.2.0")
}
