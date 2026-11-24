import java.io.File
import java.io.InputStreamReader
import java.nio.charset.StandardCharsets
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

val versionPropsFile = rootProject.file("../version.properties")
val versionProps = Properties().apply {
    versionPropsFile.inputStream().use { stream ->
        load(InputStreamReader(stream, StandardCharsets.UTF_8))
    }
}
val wsVersionName = versionProps.getProperty("VERSION_NAME")?.trim()
    ?: error("VERSION_NAME missing in ${versionPropsFile.absolutePath}")
val wsVersionDisplay = versionProps.getProperty("VERSION_DISPLAY")?.trim()
    ?: error("VERSION_DISPLAY missing in ${versionPropsFile.absolutePath}")
val wsVersionCode = versionProps.getProperty("VERSION_CODE")?.trim()?.toIntOrNull()
    ?: error("VERSION_CODE invalid in ${versionPropsFile.absolutePath}")

val xposedPropFile = project.file("src/main/resources/META-INF/xposed/module.prop")
if (xposedPropFile.exists()) {
    val expectedDesc = "寰宇工作台 $wsVersionDisplay · 局域网连接门控剪贴板手势（SystemUI）与 OPPO 分享直传（OShare）模块"
    val origText = xposedPropFile.readText(StandardCharsets.UTF_8)
    val updatedLines = origText.lines().dropLastWhile { it.isEmpty() }.map { line ->
        when {
            line.startsWith("version=") -> "version=$wsVersionName"
            line.startsWith("versionCode=") -> "versionCode=$wsVersionCode"
            line.startsWith("description=") -> "description=$expectedDesc"
            else -> line
        }
    }
    val newText = updatedLines.joinToString("\n") + "\n"
    if (origText != newText) {
        xposedPropFile.writeText(newText, StandardCharsets.UTF_8)
    }
}

val ossNoticesDir = project.file("build/generated/oss_notices")
ossNoticesDir.mkdirs()
mapOf(
    rootProject.file("../LICENSE") to File(ossNoticesDir, "LICENSE.txt"),
    rootProject.file("../THIRD_PARTY_NOTICES.md") to File(ossNoticesDir, "THIRD_PARTY_NOTICES.md"),
).forEach { (src, dst) ->
    if (src.exists()) {
        val srcText = src.readText(StandardCharsets.UTF_8)
        if (!dst.exists() || dst.readText(StandardCharsets.UTF_8) != srcText) {
            dst.writeText(srcText, StandardCharsets.UTF_8)
        }
    }
}

android {
    namespace = "me.edfork.workspace"
    sourceSets {
        getByName("main").assets.srcDir(ossNoticesDir)
    }
    compileSdk {
        version = release(37)
    }

    defaultConfig {
        applicationId = "me.edfork.workspace"
        minSdk = 31
        targetSdk = 36
        versionCode = wsVersionCode
        versionName = wsVersionName
        buildConfigField("String", "VERSION_DISPLAY", "\"$wsVersionDisplay\"")
    }

    buildTypes {
        release {
            optimization {
                enable = false
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

    testOptions {
        unitTests.isReturnDefaultValues = true
    }

    packaging {
        resources.excludes += "/META-INF/{AL2.0,LGPL2.1}"
    }
}

dependencies {
    compileOnly(libs.libxposed.api)
    implementation(libs.libxposed.service)

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.compose.runtime)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.animation)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)

    testImplementation(libs.junit)
}
