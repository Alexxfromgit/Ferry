package com.xenvaultinteractive.ferry

import android.graphics.Color
import android.os.Bundle
import androidx.activity.OnBackPressedCallback
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    // The launch splash is the night background with no icon (Theme.ferry.Starting); the web UI
    // draws the branded intro once it loads, so the two meet without a flash. The WebView is
    // night until then too: Tauri applies tauri.conf.json's backgroundColor to it.
    installSplashScreen()
    // Ferry's UI is always dark, so the system bars keep light icons even in light mode.
    enableEdgeToEdge(
      statusBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
      navigationBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
    )
    super.onCreate(savedInstanceState)
    // Back sends Ferry to the background instead of finishing the activity. Ferry has one screen
    // and no back stack, downloads keep running, and the Tauri runtime aborts the process when
    // its activity is destroyed. Registered after the plugins' callbacks, so it takes precedence.
    onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
      override fun handleOnBackPressed() {
        moveTaskToBack(true)
      }
    })
  }
}
