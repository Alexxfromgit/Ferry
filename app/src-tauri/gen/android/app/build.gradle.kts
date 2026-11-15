import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// `play` or `direct`; see ferryDistribution in buildSrc.
val ferryDist = ferryDistribution(project)

// Release signing: keystore.properties (gitignored, next to this file's parent) or environment
// variables. Each channel has its own key; Google Play re-signs the Play build with its app
// signing key. Without a key, release builds are left unsigned.
val keystoreProperties = Properties().apply {
    val propFile = rootProject.file("keystore.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}
fun signingValue(key: String, env: String): String? =
    keystoreProperties.getProperty(key) ?: System.getenv(env)
val signingPrefix = if (ferryDist == "direct") "DIRECT" else "UPLOAD"
val signingKey = if (ferryDist == "direct") "direct" else "upload"

android {
    compileSdk = 37
    namespace = "com.xenvaultinteractive.ferry"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "com.xenvaultinteractive.ferry"
        // Android 10: Ferry saves into the shared Downloads collection (MediaStore.Downloads).
        minSdk = 29
        targetSdk = 37
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
        buildConfigField("String", "FERRY_DIST", "\"$ferryDist\"")
        if (ferryDist == "direct") {
            applicationIdSuffix = ".direct"
        }
    }
    signingConfigs {
        create("release") {
            signingValue("${signingKey}StoreFile", "ANDROID_${signingPrefix}_KEYSTORE")?.let {
                storeFile = rootProject.file(it)
            }
            storePassword = signingValue("${signingKey}StorePassword", "ANDROID_${signingPrefix}_KEYSTORE_PASSWORD")
            keyAlias = signingValue("${signingKey}KeyAlias", "ANDROID_${signingPrefix}_KEY_ALIAS")
            keyPassword = signingValue("${signingKey}KeyPassword", "ANDROID_${signingPrefix}_KEY_PASSWORD")
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            signingConfig = signingConfigs.getByName("release").takeIf { it.storeFile?.exists() == true }
            optimization {
               enable = true
            }
            proguardFiles(
                *fileTree(".") {
                  include("**/*.pro")
                  exclude("build/**")
                }.files.toTypedArray()
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    buildFeatures {
        buildConfig = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}

rust {
    rootDirRel = "../../../"
}

// Channel-specific code, resources and manifest entries live in src/play and src/direct.
androidComponents {
    onVariants { variant ->
        variant.sources.java?.addStaticSourceDirectory("src/$ferryDist/java")
        variant.sources.res?.addStaticSourceDirectory("src/$ferryDist/res")
        val manifest = file("src/$ferryDist/AndroidManifest.xml")
        if (manifest.exists()) {
            variant.sources.manifests.addStaticManifestFile(manifest.path)
        }
    }
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
    // Google Play's in-app updates, only in the Play build (see src/play).
    if (ferryDist == "play") {
        implementation("com.google.android.play:app-update-ktx:2.1.0")
    }
}

apply(from = file("tauri.build.gradle.kts"))
