package com.xenvaultinteractive.ferry

import android.os.Bundle
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
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
