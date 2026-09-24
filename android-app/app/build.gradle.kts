import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    // Cross-compiles the `call-core` Rust crate (call/pairing state
    // machine, including SPAKE2 pairing via its `pake-bridge` dependency)
    // and drops the resulting .so files into this module's jniLibs, wired
    // below so plain `./gradlew assembleDebug` builds it automatically —
    // no separate manual "build the Rust part first" step.
    id("org.mozilla.rust-android-gradle.rust-android") version "0.10.0"
}

android {
    namespace = "dev.porchlight.app"
    // Portal targets old AOSP (targetSdk 29), but we compile against whatever
    // platform is installed locally. 34+ is fine; only targetSdk affects runtime.
    compileSdk = 36
    buildToolsVersion = "35.0.0"
    // Pinned explicitly (not auto-detected) so both AGP's own native build
    // support and the rust-android-gradle plugin below resolve the exact
    // same NDK deterministically. Install via:
    //   sdkmanager --install "ndk;27.3.13750724"
    ndkVersion = "27.3.13750724"

    defaultConfig {
        applicationId = "dev.porchlight.app"
        // Portal devices run older AOSP. minSdk 28, target 29 per Portal guidance.
        minSdk = 28
        targetSdk = 29
        versionCode = 7
        versionName = "0.6"

        // Quartz's secp256k1 crypto is JNI-native — the Portal only ever
        // reports arm64-v8a/armeabi-v7a (see
        // `adb shell getprop ro.product.cpu.abilist`). x86_64 isn't a real
        // device ABI at all — it's added only for the debug build type
        // below, so the app installs and runs on the Android emulator for
        // UI verification (this machine's emulator can't run arm64 images
        // on an x86_64 host). Release only ever ships what real Portal
        // hardware can use.
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a")
        }

        // Where UpdateChecker.kt looks for new releases — a GitHub
        // "owner/repo" slug, queried as
        // https://api.github.com/repos/<UPDATE_REPO>/releases/latest (see
        // that file's own doc for the release-asset naming convention it
        // expects). Same gitignored-local.properties-or-gradle-property
        // pattern as DEFAULT_DEVICE_NAME below — never committed, since
        // which repo this points at is a per-deployment choice. Empty
        // means "update checking is off": UpdateChecker treats a blank
        // value as nothing to check against.
        val updateProps = Properties().apply {
            val f = rootProject.file("local.properties")
            if (f.exists()) f.inputStream().use { load(it) }
        }
        val updateRepo = (project.findProperty("porchlightUpdateRepo") as String?)
            ?: updateProps.getProperty("porchlight.updateRepo")
            ?: ""
        buildConfigField("String", "UPDATE_REPO", "\"$updateRepo\"")
    }

    // Release signing. If a release keystore is configured (via local.properties
    // keys release.storeFile/storePassword/keyAlias/keyPassword, or the matching
    // RELEASE_STORE_FILE/… env vars), use it. Otherwise fall back to the debug
    // key so `assembleRelease` still produces an installable APK for sideloading
    // onto the device — never block a release build on a missing keystore.
    val releaseProps = Properties().apply {
        val f = rootProject.file("local.properties")
        if (f.exists()) f.inputStream().use { load(it) }
    }
    fun releaseProp(key: String, env: String): String? =
        (project.findProperty(key) as String?)
            ?: releaseProps.getProperty(key)
            ?: System.getenv(env)
    val releaseStorePath = releaseProp("release.storeFile", "RELEASE_STORE_FILE")
    val hasReleaseKeystore = releaseStorePath != null && rootProject.file(releaseStorePath).exists()

    signingConfigs {
        if (hasReleaseKeystore) {
            create("release") {
                storeFile = rootProject.file(releaseStorePath!!)
                storePassword = releaseProp("release.storePassword", "RELEASE_STORE_PASSWORD")
                keyAlias = releaseProp("release.keyAlias", "RELEASE_KEY_ALIAS")
                keyPassword = releaseProp("release.keyPassword", "RELEASE_KEY_PASSWORD")
            }
        }
    }

    lint {
        // targetSdk 29 is intentional (Portal runs old AOSP) — don't fail the
        // release build's lintVital check on the expected "expired target SDK".
        disable += "ExpiredTargetSdkVersion"
    }

    buildTypes {
        debug {
            // x86_64 only, on top of defaultConfig's real-device ABIs — see
            // that block's own doc. Debug-only because it exists purely for
            // running on this machine's x86_64 emulator.
            ndk {
                abiFilters += listOf("x86_64")
            }
            // Local convenience: pre-fill this device's name so a
            // fresh debug install skips the name-entry screen. Set it in the
            // gitignored local.properties (porchlight.deviceName=...) or pass
            // -PporchlightDeviceName=… — never committed. Defaults to empty.
            val localProps = Properties().apply {
                val f = rootProject.file("local.properties")
                if (f.exists()) f.inputStream().use { load(it) }
            }
            val devName = (project.findProperty("porchlightDeviceName") as String?)
                ?: localProps.getProperty("porchlight.deviceName")
                ?: ""
            buildConfigField("String", "DEFAULT_DEVICE_NAME", "\"$devName\"")
        }
        release {
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            buildConfigField("String", "DEFAULT_DEVICE_NAME", "\"\"")
            // Use the release keystore if configured, else the debug key so the
            // APK is always installable (sideload-only app, no Play Store).
            // The fallback is intentional, but silent otherwise — a CI
            // runner or a local.properties mishap could ship a debug-signed
            // "release" with no visible signal, which matters here
            // specifically because Android's own signature check is this
            // app's entire update-integrity model (see UpdateChecker.kt's
            // own doc). The warning below is the fix: loud in the build
            // log, but still never blocks the build itself.
            signingConfig = if (hasReleaseKeystore) {
                signingConfigs.getByName("release")
            } else {
                logger.warn("assembleRelease: no release keystore configured (release.storeFile/RELEASE_STORE_FILE) — falling back to the DEBUG signing key. This APK is fine for your own sideload, but must not be distributed as a real release: installing it over a properly-signed prior release will fail, and anyone relying on Android's signature check to trust future updates gets no such guarantee from a debug-signed build.")
                signingConfigs.getByName("debug")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        compose = true
        buildConfig = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

// One-time setup this needs on a fresh machine (not run automatically —
// same "documented manual command" posture as this project's pinned
// JAVA_HOME requirement):
//   rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
//   sdkmanager --install "ndk;27.3.13750724"   # or record whatever exact
//                                               # version was actually used
// "arm"/"arm64"/"x86_64" below are this plugin's own target aliases for
// armeabi-v7a/arm64-v8a/x86_64 — the union of defaultConfig's real-device
// ABIs and debug's added x86_64 above, on purpose. Cargo builds all three
// for any variant regardless (this plugin isn't variant-aware); Android's
// own per-buildType abiFilters is what actually keeps x86_64 out of a
// release APK.
cargo {
    module = "../../call-core"
    libname = "call_core"
    targets = listOf("arm", "arm64", "x86_64")
    // The plugin's own default for this is "$module/target" — correct for
    // a standalone crate, but wrong now that call-core is a workspace
    // member: Cargo puts the actual build output in the *workspace root's*
    // target/ dir, not call-core/target (which doesn't even exist). Left
    // unset, the plugin's Copy task silently copies zero files — no error,
    // just an empty jniLibs dir and a broken APK that still builds
    // successfully. Must stay in sync with the `[workspace]` in the
    // repo-root Cargo.toml.
    targetDirectory = "../../target"
    // Left at the plugin's default (== minSdk, 28) rather than pinned here
    // separately — one fewer place for the two to silently drift apart.

    // The plugin's linker wrapper shells out to a "python" command that
    // doesn't exist on a plain macOS install (only python3 does, since
    // Python 2 was removed). Pinned here as committed project config
    // rather than left as a per-developer environment variable someone
    // has to remember to set.
    pythonCommand = "python3"
}

// Makes `./gradlew assembleDebug` (and any other normal build) compile the
// Rust side automatically, the same way it already compiles Kotlin — no
// separate manual "build the Rust part first" step for anyone after the
// one-time toolchain setup above. `javaPreCompileDebug`/`javaPreCompileRelease`
// is this plugin's own documented hook point, chosen deliberately over
// hooking a later task: it needs to run early enough that the .so it
// produces is in place before Android's own jniLibs merge step looks for it.
//
// styleDictionaryBuildAndroid is *also* wired directly onto
// compileDebugKotlin/compileReleaseKotlin below, not just onto
// javaPreCompileDebug/Release here — found live in CI (not locally, where
// it passed every time): a real build failed with "Unresolved reference
// 'Dimens'"/'GeneratedColor'/'GeneratedOpacity', meaning compileDebugKotlin
// ran before styleDictionaryBuildAndroid had regenerated those files, even
// though javaPreCompileDebug supposedly depends on it. Gradle's task graph
// doesn't transitively guarantee "everything javaPreCompileDebug depends on
// finishes before every *other* task that isn't itself ordered after
// javaPreCompileDebug" — compileDebugKotlin has no dependency relationship
// on javaPreCompileDebug at all, so nothing actually forced the order
// between them. Depending on styleDictionaryBuildAndroid directly from the
// Kotlin compile tasks closes that gap instead of relying on an indirect
// path that happened to work locally by luck of task scheduling.
// mergeReleaseJniLibFolders needs the exact same direct-dependency
// treatment as compileReleaseKotlin above, for the exact same reason —
// found live on real Portal TV hardware (not caught by any emulator
// testing, which only ever installs debug builds): a real assembleRelease
// scheduled mergeReleaseJniLibFolders/mergeReleaseNativeLibs *before*
// cargoBuild even started, so the release APK packaged zero bytes of
// libcall_core.so for every ABI — installs fine, then crashes on first
// launch with UnsatisfiedLinkError the moment CallCoreBridge's static
// initializer runs, and this app's own crash handler (PorchlightApplication)
// restarts it, producing an infinite relaunch loop. The debug variant's
// mergeDebugJniLibFolders happened to already run after cargoBuild in
// practice, which is exactly the "worked locally by luck of scheduling"
// trap this file already has one prior example of.
tasks.whenTaskAdded {
    if (name == "javaPreCompileDebug" || name == "javaPreCompileRelease") {
        dependsOn("cargoBuild")
        dependsOn("styleDictionaryBuildAndroid")
    }
    if (name == "compileDebugKotlin" || name == "compileReleaseKotlin") {
        dependsOn("styleDictionaryBuildAndroid")
    }
    if (name == "mergeDebugJniLibFolders" || name == "mergeReleaseJniLibFolders") {
        dependsOn("cargoBuild")
    }
}

// Ensures ../../tokens/node_modules exists before styleDictionaryBuildAndroid
// runs. `npx --yes style-dictionary` downloads its own *separate*,
// throwaway copy of the package into npx's own cache just to run the
// `style-dictionary` CLI binary — but `config/android.config.mjs` itself
// has its own plain ESM `import StyleDictionary from 'style-dictionary'`
// line, which Node resolves via ordinary module resolution starting from
// that *file's own* directory (walking up through tokens/node_modules), a
// completely different lookup path npx's isolated cache is invisible to.
// So this always needs a real `tokens/node_modules/style-dictionary` to
// exist, regardless of npx's own cache state.
//
// `inputs`/`outputs` declared (not just an unconditional Exec) so this is
// skipped on every later build once already up to date — `npm ci` itself
// has no incremental fast-path of its own, and this task's own dependents
// re-run on every build (see cargoBuild's own doc above), so without this
// every single local `./gradlew assembleDebug` would pay a multi-second
// `npm ci` tax for no reason. `npm ci`, not `npm install`: reproducible
// from the committed ../../tokens/package-lock.json, and correct to fail
// loudly if that lockfile and package.json ever drift out of sync, rather
// than silently updating the lockfile the way `npm install` would.
tasks.register<Exec>("npmInstallTokens") {
    val tokensDir = rootProject.file("../tokens")
    workingDir = tokensDir
    // Absolute (via tokensDir), not a string like "../tokens/package-lock.json"
    // — Exec's inputs/outputs resolve relative to *this* project directory
    // (android-app/app/), not workingDir, so a path written relative to
    // workingDir silently points at the wrong file (android-app/tokens/...,
    // which doesn't exist) and fails task validation outright.
    inputs.file(File(tokensDir, "package-lock.json"))
    outputs.dir(File(tokensDir, "node_modules"))
    commandLine("npm", "ci")
}

// Regenerates Dimens.kt/GeneratedColor.kt/GeneratedType.kt in ui/theme/ from
// the shared design-token source in ../../tokens (kept in sync with the web
// client's own generated tokens.css) — same "run automatically, no separate
// manual step" posture as cargoBuild above. One-time setup this needs on a
// fresh machine (not run automatically, same posture as this project's
// pinned JAVA_HOME/rustup/sdkmanager requirements): install Node.js (LTS;
// this project doesn't pin an exact version yet) so `npm`/`npx` resolve.
tasks.register<Exec>("styleDictionaryBuildAndroid") {
    dependsOn("npmInstallTokens")
    workingDir = rootProject.file("../tokens")
    commandLine("npx", "--yes", "style-dictionary", "build", "--config", "config/android.config.mjs")
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.service)
    implementation(libs.androidx.activity.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.ui)
    implementation(libs.androidx.ui.graphics)
    implementation(libs.androidx.ui.tooling.preview)
    implementation(libs.androidx.material3)
    implementation(libs.androidx.tv.material)
    implementation(libs.webrtc)
    implementation(libs.coroutines.android)
    implementation(libs.quartz.android)
    implementation(libs.okhttp.android)
}
