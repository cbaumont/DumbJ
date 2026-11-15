plugins {
    alias(libs.plugins.kotlin.jvm)
}

dependencies {
    implementation(libs.kotlinx.coroutines.core)
    implementation(libs.okio)
    testImplementation(libs.kotlinx.coroutines.test)
    println(libs.versions.kotlin.get())
}
