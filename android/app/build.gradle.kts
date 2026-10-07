import java.util.Properties

plugins { id("com.android.application"); id("org.jetbrains.kotlin.android"); id("org.jetbrains.kotlin.plugin.compose"); id("com.google.devtools.ksp") }
// Private deployment defaults survive local rebuilds; environment values take precedence.
val releaseConnection = Properties().apply {
 val config = rootProject.file("../local/release-connection.properties")
 if (config.isFile) config.inputStream().use { load(it) }
}
android {
 namespace = "dev.threadbridge"
 compileSdk = 35
 defaultConfig { applicationId = "dev.threadbridge"; minSdk = 26; targetSdk = 35; versionCode = 22; versionName = "0.1.21-test" }
 defaultConfig {
  for (field in listOf("PUBLIC_TEST_SERVER", "PUBLIC_TEST_PREVIOUS_SERVER", "PUBLIC_TEST_HOST")) {
   val value = System.getenv("THREADBRIDGE_$field") ?: releaseConnection.getProperty(field, "")
   require(value.matches(Regex(if (field == "PUBLIC_TEST_PREVIOUS_SERVER") "[a-zA-Z0-9:/._,-]*" else "[a-zA-Z0-9:/._-]*"))) { "Invalid $field" }
   buildConfigField("String", field, "\"$value\"")
  }
 }
 buildFeatures { compose = true; buildConfig = true }
 compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
 kotlinOptions { jvmTarget = "17" }
 signingConfigs { create("personal") { val key = System.getenv("THREADBRIDGE_KEYSTORE"); if (key != null) { storeFile = file(key); storePassword = System.getenv("THREADBRIDGE_KEY_PASSWORD"); keyAlias = "threadbridge"; keyPassword = System.getenv("THREADBRIDGE_KEY_PASSWORD") } } }
 buildTypes { getByName("release") { isMinifyEnabled = false; signingConfig = signingConfigs.getByName("personal") }; getByName("debug") { applicationIdSuffix = ".debug" } }
}
ksp { arg("room.schemaLocation", "$projectDir/schemas") }
dependencies {
 implementation(platform("androidx.compose:compose-bom:2025.04.01"))
 implementation("androidx.activity:activity-compose:1.10.1")
 implementation("androidx.compose.ui:ui")
 implementation("androidx.compose.foundation:foundation")
 implementation("androidx.compose.material3:material3")
 implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
 implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
 implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
 implementation("androidx.room:room-runtime:2.7.1")
 implementation("androidx.room:room-ktx:2.7.1")
 ksp("androidx.room:room-compiler:2.7.1")
 implementation("com.squareup.okhttp3:okhttp:4.12.0")
 implementation("com.journeyapps:zxing-android-embedded:4.3.0")
 implementation("org.commonmark:commonmark:0.30.0")
 implementation("org.commonmark:commonmark-ext-gfm-tables:0.30.0")
 testImplementation("junit:junit:4.13.2")
}
