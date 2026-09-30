buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:8.11.0")
        // Kotlin 编译器版本必须 >= 传递依赖拉入的 kotlin-stdlib 版本（当前 2.2.21），
        // 否则编译 WryActivity.kt 会报 "compiled with an incompatible version of Kotlin"。
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.2.21")
    }
}

allprojects {
    repositories {
        google()
        mavenCentral()
    }
}

tasks.register("clean").configure {
    delete("build")
}

