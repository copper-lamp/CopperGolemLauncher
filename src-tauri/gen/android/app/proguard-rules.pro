# CopperGolem Android game host keep rules.
#
# The Minecraft Bedrock host is entered from native code and through reflection
# (AssetManager#addAssetPath, JNI method binding on com.mojang.minecraftpe.*),
# so neither R8 nor the resource shrinker may rename or drop it.

-keep class com.mojang.** { *; }
-keep class com.microsoft.xal.** { *; }
-keep class com.microsoft.xbox.** { *; }
-keep class com.microsoft.applications.** { *; }
-keep class com.microsoft.playfab.** { *; }
-keep class com.madgag.spongycastle.** { *; }
-keep class org.conscrypt.** { *; }
-keep class org.fmod.** { *; }

-keep class com.copperlamp.coppergolem.game.** { *; }

# JNI entry points resolved by name from libminecraftpe.so / libgxcore.so.
-keepclasseswithmembernames,includedescriptorclasses class * {
    native <methods>;
}

# Room generated implementations are referenced reflectively by Room_Impl lookup.
-keep class * extends androidx.room.RoomDatabase { <init>(); }
-keep @androidx.room.Entity class * { *; }

# Manifest-declared Mojang components instantiated by the framework.
-keep class com.mojang.minecraftpe.ImportService { *; }
-keep class com.mojang.minecraftpe.NotificationListenerService { *; }

-keepattributes SourceFile,LineNumberTable,Signature,Exceptions,InnerClasses,EnclosingMethod,*Annotation*,RuntimeVisibleAnnotations,AnnotationDefault
-dontwarn org.conscrypt.**
-dontwarn com.madgag.spongycastle.**
-dontwarn javax.naming.**
-dontwarn org.bouncycastle.**
-dontwarn org.openjsse.**
-dontwarn com.microsoft.**
-dontwarn com.mojang.**
