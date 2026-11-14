# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile

# ML Kit (the QR scanner) creates its components through their no-argument constructors, found
# by reflection. R8's full mode keeps the classes but drops those constructors, and the scanner
# then crashes the app on its first camera frame.
-keepclassmembers class * implements com.google.firebase.components.ComponentRegistrar {
    public <init>();
}

# Tauri loads Ferry's update plugin by class name (see src/direct and src/play).
-keep class com.xenvaultinteractive.ferry.UpdatePlugin { *; }
-keep class com.xenvaultinteractive.ferry.UpdatePlugin$* { *; }
