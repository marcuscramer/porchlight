# Not currently applied — release builds run with isMinifyEnabled = false
# (see app/build.gradle.kts). Kept ready for whenever that changes.

# Keep WebRTC native bridge classes (accessed via JNI).
-keep class org.webrtc.** { *; }
-dontwarn org.webrtc.**

# Keep CallCoreBridge (call-core Rust crate's JNI bridge, see its own doc) —
# its `external fun`s are matched to `Java_dev_porchlight_app_
# CallCoreBridge_native*` symbols by name convention, not RegisterNatives,
# so R8 renaming the class or its methods would silently break every
# native call the moment minification is turned on.
-keep class dev.porchlight.app.CallCoreBridge { *; }

# Ably (gson, msgpack, vcdiff transitive deps)
-dontwarn io.ably.**
-dontwarn com.google.gson.**
-dontwarn org.msgpack.**
-dontwarn com.davidehrmann.vcdiff.**
